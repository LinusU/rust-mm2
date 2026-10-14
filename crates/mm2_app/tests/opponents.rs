//! F15-A.2 opponent integration: an authored `[Opponent]` roster on a
//! synthetic install spawns real AI participants through the production
//! `load_session_world` → `opponent_roster` → `load_opponent` path —
//! each with its own vehicle config, `PlayerControl::Ai`, `RaceProgress`
//! and session ownership — then `opponent_drive` chases the authored
//! `.opp` routes through the same `VehicleInput` → physics →
//! `advance_race` validation the player's controls feed.

use crate::support;

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::opponents::{
    DriveStats, OpponentDriver, REANCHOR_CLEAR, REANCHOR_FRAMES, circuit_lap, driving_route,
    initial_route_index, opponent_drive, reanchor_pose, reanchor_pose_with_progress,
    route_is_closed, spawn_pose, trim_behind_staging,
};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts, race};
use mm2_assets::Vfs;
use mm2_game::{
    Densities, Difficulty, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, OpponentRoute,
    OpponentRoutePoint, OpponentSpec, ParticipantState, Player, PlayerControl, PlayerVehicle,
    RaceCustomization, RaceDefinition, RaceProgress, RaceStarted, RaceState, ResultLedger,
    RouteGateLine, Session, SessionAuthority, SessionConditions, SessionConfig,
    SessionCustomization, SessionEntity, SessionMode, SessionPhase, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig, VehicleInput, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

/// The same dev-world lane `tests/event.rs` uses: gates at x=110/140/165,
/// finish at x=180, all at z=140 with a 15 m trigger radius — an
/// opponent lane at z=146 still sweeps them.
pub(crate) const COURSE_Z: f32 = 140.0;

pub(crate) fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

pub(crate) fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

/// One vehicle: the shared `tuned_car` base (tune + model + bound —
/// `tests/support`'s fixture, the same grammar `audio_car` layers
/// cardata on) plus a retail-style `_opp` tune file unless `opp_mass` is
/// None — written so the tests can prove an opponent never reads it.
pub(crate) fn write_car(d: &Path, id: &str, mass: f32, opp_mass: Option<f32>) {
    support::tuned_car(d, id, mass);
    if let Some(m) = opp_mass {
        write(
            d,
            &format!("tune/vehicle/{id}_opp.vehcarsim"),
            support::vehcarsim(m),
        );
    }
}

fn opp_file(points: &[[f32; 3]]) -> String {
    let mut s = OPP_HEADER.to_string();
    for p in points {
        s.push_str(&format!("{},{},{},0,0,0,0,0,0\n", p[0], p[1], p[2]));
    }
    s
}

fn aimap_with_opponents(rows: &str) -> String {
    let n = rows.lines().filter(|l| !l.trim().is_empty()).count();
    format!("[Opponent]\n{n}\n{rows}")
}

pub(crate) fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},-90,15,0,0,0,\n")
}

/// A `race/testcity/` checkpoint event wiring two opponents on parallel
/// lanes (`vpt` at z=140, `vpheavy` at z=146) plus the vehicle files
/// `load_opponent` resolves through the same VFS.
fn roster_install(extra_aimap_rows: &str, extra_files: &[(&str, String)]) -> tempfile::TempDir {
    roster_install_rows(
        &format!(
            "vpt race0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\nvpheavy race0-a-1.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n{extra_aimap_rows}"
        ),
        extra_files,
    )
}

/// The same install with a caller-authored `[Opponent]` body — the
/// production parameter-tail test feeds real tail variants through it.
fn roster_install_rows(rows: &str, extra_files: &[(&str, String)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,2,0,0.1,0.0,1,50,1,0,0,0,2,0,0.2,0.0,1,40,1\n"),
    );
    write(d, "race/testcity/race0.aimap", aimap_with_opponents(rows));
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}{}",
            waypoint_row(60.0, COURSE_Z),
            waypoint_row(110.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(165.0, COURSE_Z),
            waypoint_row(180.0, COURSE_Z),
        ),
    );
    write(
        d,
        "race/testcity/race0-a-0.opp",
        opp_file(&[
            [70.0, 0.0, 140.0],
            [110.0, 0.0, 140.0],
            [140.0, 0.0, 140.0],
            [165.0, 0.0, 140.0],
            [180.0, 0.0, 140.0],
        ]),
    );
    write(
        d,
        "race/testcity/race0-a-1.opp",
        opp_file(&[
            [70.0, 0.0, 146.0],
            [110.0, 0.0, 146.0],
            [140.0, 0.0, 146.0],
            [165.0, 0.0, 146.0],
            [180.0, 0.0, 146.0],
        ]),
    );
    // `vpt` ships a retail-style `_opp` tune (mass 2000 vs base 1000)
    // the original never reads; `vpheavy` (mass 3000) has none.
    write_car(d, "vpt", 1000.0, Some(2000.0));
    write_car(d, "vpheavy", 3000.0, None);
    for (rel, contents) in extra_files {
        write(d, &format!("race/testcity/{rel}"), contents);
    }
    tmp
}

pub(crate) fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// A `race/testcity/` Circuit event: `circuit0` (2 opponents, NumLaps
/// 2), the dev-lane course as the closed route, an authored `cir0`
/// grid and two opponents chasing closed `.opp` loops — the roster +
/// Ordered + laps combination F14-AC05's restart leg needs.
fn circuit_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,2,0,0.1,0.0,2,50,1,0,0,0,2,0,0.2,0.0,2,40,1\n"),
    );
    write(
        d,
        "race/testcity/circuit0.aimap",
        aimap_with_opponents(
            "vpt circuit0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n\
             vpheavy circuit0-a-1.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n",
        ),
    );
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(60.0, COURSE_Z), // start line — the lifted copy closes each lap
            waypoint_row(110.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(165.0, COURSE_Z),
        ),
    );
    // Authored grid — player + two opponent slots, `a = 0` (no
    // heading, the retail `cir6` shape): facing derives off the course.
    write(
        d,
        "race/testcity/cir0_strtpnts",
        "60,0,140,0,0,0,0,0,0,\n56,0,144,0,0,0,0,0,0,\n52,0,148,0,0,0,0,0,0,\n",
    );
    // Closed loops down and back along the lane — each last anchor
    // lands within `ROUTE_LOOP` of its first.
    write(
        d,
        "race/testcity/circuit0-a-0.opp",
        opp_file(&[
            [70.0, 0.0, 140.0],
            [180.0, 0.0, 140.0],
            [180.0, 0.0, 146.0],
            [70.0, 0.0, 146.0],
            [72.0, 0.0, 140.0],
        ]),
    );
    write(
        d,
        "race/testcity/circuit0-a-1.opp",
        opp_file(&[
            [70.0, 0.0, 146.0],
            [180.0, 0.0, 146.0],
            [180.0, 0.0, 140.0],
            [70.0, 0.0, 140.0],
            [72.0, 0.0, 146.0],
        ]),
    );
    write_car(d, "vpt", 1000.0, None);
    write_car(d, "vpheavy", 3000.0, None);
    tmp
}

fn circuit_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Circuit,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The headless_smoke system set plus `opponent_drive` — the real
/// session/race drivers on a minimal app.
pub(crate) fn event_app(config: SessionConfig, vfs: Vfs) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(f64::from(mm2_game::RACE_TICK_HZ)))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
                race::reanchor_teleported_participants,
                race::advance_race,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
                race::update_checkpoint_markers,
                opponent_drive,
            ),
        );
    app.finish();
    app.cleanup();
    app
}

pub(crate) fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

fn opponents(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<OpponentDriver>>()
        .iter(app.world())
        .collect()
}

fn opponent_by_vehicle(app: &mut App, id: &str) -> Entity {
    app.world_mut()
        .query_filtered::<(Entity, &OpponentDriver), ()>()
        .iter(app.world())
        .find(|(_, d)| d.spec.vehicle == id)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no opponent driving {id}"))
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn route(points: &[[f32; 3]]) -> OpponentRoute {
    OpponentRoute {
        points: points
            .iter()
            .map(|p| OpponentRoutePoint {
                position: Vec3::new(p[0], p[1], p[2]),
                brake: 0.0,
                forward_offset: 0.0,
                side_offset: 0.0,
                target_speed: 0.0,
                speed_start: 0.0,
                side_start: 0.0,
            })
            .collect(),
    }
}

/// A route whose row-0 `brake` carries the authored staging heading —
/// the measured meaning of a nonzero value on retail `.opp` files.
fn staged_route(heading_deg: f32, points: &[[f32; 3]]) -> OpponentRoute {
    let mut r = route(points);
    r.points[0].brake = heading_deg;
    r
}

/// A circuit record's lap is its intersection rows (`0x55c2d0`: the
/// route index wraps to row 1). The start pose and the finish position
/// stand on the lap; closing the loop through them would send the car
/// back and forth past its own grid every lap.
#[test]
fn a_circuit_record_reduces_to_its_lap_of_intersections() {
    // Retail `sf circuit8` in miniature: staged at x = −244 facing −X on
    // the road from row 1 (x = −113, behind the grid) to row 2
    // (x = −351); the last intersection is row 1's junction again and
    // the finish position sits just past the start.
    let record = staged_route(
        90.0,
        &[
            [-244.0, 0.0, -288.0],
            [-113.0, 0.0, -285.0],
            [-351.0, 0.0, -283.0],
            [-351.0, 0.0, -100.0],
            [-113.0, 0.0, -100.0],
            [-126.0, 0.0, -287.0],
            [-255.0, 0.0, -290.0],
        ],
    );
    let lap = circuit_lap(&record).expect("a circuit record");
    let xs: Vec<f32> = lap.points.iter().map(|p| p.position.x).collect();
    assert_eq!(
        xs,
        [-113.0, -351.0, -351.0, -113.0, -126.0, -113.0],
        "rows 1..n-2, closed on the first intersection"
    );
    assert!(route_is_closed(&lap));
    // The car, staged between rows 1 and 2 facing row 2, heads for it.
    let start = record.points[0].position;
    let yaw = record.start_heading_deg().unwrap().to_radians();
    assert_eq!(initial_route_index(&lap, start, yaw), 1);

    // The driving line is the lap: nothing in it doubles back onto the
    // staging or finish rows.
    let driven = driving_route(&record, None, &[], &[], start, yaw);
    assert_eq!(driven.points.len(), lap.points.len());

    // An open record, and a closed one too short to hold a lap, are
    // left to the staging trim.
    let open = route(&[
        [0.0, 0.0, 0.0],
        [-50.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [200.0, 0.0, 0.0],
        [300.0, 0.0, 0.0],
        [400.0, 0.0, 0.0],
    ]);
    assert!(circuit_lap(&open).is_none());
    let short = route(&[
        [0.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [100.0, 0.0, 100.0],
        [0.0, 0.0, 10.0],
    ]);
    assert!(circuit_lap(&short).is_none());
}

/// An opponent stages on its own route — the original places every
/// opponent at its `.opp` row 0 and reads no start grid (UNK-17). A
/// grid slot seats only an entry with no route to stage on.
#[test]
fn spawn_pose_stages_on_the_route_then_the_grid() {
    let mut def = RaceDefinition {
        checkpoints: Vec::new(),
        finish: None,
        rule: mm2_game::CheckpointRule::AnyOrder,
        laps: 0,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![
            mm2_game::RaceStart {
                position: Vec3::new(60.0, 0.0, 140.0),
                yaw_deg: Some(90.0),
            },
            mm2_game::RaceStart {
                position: Vec3::new(56.0, 0.0, 144.0),
                yaw_deg: Some(90.0),
            },
        ],
    };
    let spec = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: Some(route(&[[70.0, 0.0, 140.0], [110.0, 0.0, 140.0]])),
    };
    // The route's row 0 anchors the spawn, grid or no grid; with no
    // authored staging heading the first leg supplies the facing.
    let (pos, yaw) = spawn_pose(&def, 0, &spec, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 140.0), "route row0 anchors");
    assert!(
        (yaw + std::f32::consts::FRAC_PI_2).abs() < 0.35,
        "no authored heading → the +X first leg faces it, got {yaw}"
    );

    // A route-less entry takes its grid slot and the slot's authored
    // `a` — vehicle-yaw degrees.
    let bare = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: None,
    };
    let (pos, yaw) = spawn_pose(&def, 0, &bare, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(56.0, 0.0, 144.0), "authored slot 1");
    assert!(
        (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "slot spawn faces its authored yaw, got {yaw}"
    );
    def.start_slots.truncate(1);

    // No grid, no route → designed stagger behind the player.
    let bare = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: None,
    };
    let (pos, yaw) = spawn_pose(&def, 1, &bare, Vec3::new(60.0, 0.0, 140.0), 0.0);
    assert!(
        pos.z > 145.0 && (pos.x - 60.0).abs() < 1.0,
        "staggered behind a -Z-facing player, got {pos:?}"
    );
    assert_eq!(yaw, 0.0, "no route → the player's heading");
}

/// The `.opp` row-0 `brake` is the staged-start heading (measured on
/// 592/612 retail files): on a route-anchor spawn it beats the
/// first-leg direction — an authored −X facing must not flip to the
/// +X leg.
#[test]
fn spawn_pose_faces_the_authored_staging_heading() {
    let def = RaceDefinition {
        checkpoints: Vec::new(),
        finish: None,
        rule: mm2_game::CheckpointRule::AnyOrder,
        laps: 0,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![mm2_game::RaceStart {
            position: Vec3::new(60.0, 0.0, 140.0),
            yaw_deg: Some(0.0),
        }],
    };
    let spec = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: Some(staged_route(
            90.0,
            &[[70.0, 0.0, 140.0], [110.0, 0.0, 140.0]],
        )),
    };
    let (pos, yaw) = spawn_pose(&def, 0, &spec, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 140.0));
    assert!(
        (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "authored 90° staging faces −X (vehicle yaw +π/2), got {yaw}"
    );
}

/// A route stages its opponent whatever grid the event carries: the
/// staged heading when row 0 authors one, else the first leg. A grid
/// slot with no authored heading (`yaw_deg == None` — the producer's
/// `_strtpnts` `a = 0` mapping, retail `cir6`'s all-zero column) seats
/// a route-less entry facing the way the player does.
#[test]
fn spawn_pose_faces_the_route_and_a_headless_slot_the_player() {
    let def = RaceDefinition {
        checkpoints: Vec::new(),
        finish: None,
        rule: mm2_game::CheckpointRule::AnyOrder,
        laps: 0,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![
            mm2_game::RaceStart {
                position: Vec3::new(60.0, 0.0, 140.0),
                yaw_deg: None,
            },
            mm2_game::RaceStart {
                position: Vec3::new(56.0, 0.0, 144.0),
                yaw_deg: None,
            },
        ],
    };
    let fwd = |yaw: f32| Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    // Staged route: the authored 180° heading faces +Z (vehicle yaw π),
    // not the authored-0 −Z a verbatim read would produce.
    let staged = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: Some(staged_route(
            180.0,
            &[[70.0, 0.0, 144.0], [70.0, 0.0, 170.0]],
        )),
    };
    let (pos, yaw) = spawn_pose(&def, 0, &staged, Vec3::new(60.0, 0.0, 140.0), 0.0);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 144.0), "the route's row 0 stands");
    assert!(
        fwd(yaw).z > 0.98,
        "the route's 180° staging faces +Z, got {yaw}"
    );

    // No staged heading → the route's first leg supplies it: +Z.
    let unstaged = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: Some(route(&[[70.0, 0.0, 144.0], [70.0, 0.0, 170.0]])),
    };
    let (pos, yaw) = spawn_pose(&def, 0, &unstaged, Vec3::new(60.0, 0.0, 140.0), 0.0);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 144.0));
    assert!(
        fwd(yaw).z > 0.98,
        "no authored heading → the first-leg facing, got {yaw}"
    );

    // A route-less entry on a headless slot inherits the player's yaw.
    let bare = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: None,
    };
    let (pos, yaw) = spawn_pose(&def, 0, &bare, Vec3::new(60.0, 0.0, 140.0), 0.4);
    assert_eq!(pos, Vec3::new(56.0, 0.0, 144.0), "the authored slot stands");
    assert_eq!(yaw, 0.4, "no route → the player's heading");
}

/// A staged start joins the `.opp` line mid-leg (measured on retail
/// `circuit1-a-0`: the staging heading runs −X while row 1 sits +X of
/// it): the first chase target must lie ahead of the authored facing —
/// chasing the staging row's own tail U-turns the car off the line.
#[test]
fn initial_chase_index_skips_anchors_behind_the_staging() {
    let r = staged_route(
        90.0,
        &[
            [0.0, 0.0, 0.0],
            [50.0, 0.0, 0.0],  // behind a −X-facing spawn
            [-50.0, 0.0, 0.0], // down-course
            [-100.0, 0.0, 0.0],
        ],
    );
    let pos = Vec3::new(0.0, 0.0, 0.0);
    assert_eq!(
        initial_route_index(&r, pos, std::f32::consts::FRAC_PI_2),
        2,
        "facing −X the +X anchors are behind the line"
    );
    assert_eq!(
        initial_route_index(&r, pos, -std::f32::consts::FRAC_PI_2),
        1,
        "facing +X the first leg is the chase"
    );
}

/// Retail `.opp` lines author row 1 behind the grid (`sf` checkpoint
/// 2: 40 m behind a −Z-facing car). The nav re-path would join the
/// car to it by a lap around the block, so those anchors are dropped
/// *before* densifying — the staging row stays (it carries the
/// heading), the course anchors after it are untouched.
#[test]
fn anchors_behind_the_staging_are_dropped_before_densifying() {
    let r = staged_route(
        90.0,
        &[
            [0.0, 0.0, 0.0],
            [50.0, 0.0, 0.0], // behind a −X-facing spawn
            [-50.0, 0.0, 0.0],
            [-100.0, 0.0, 0.0],
        ],
    );
    let pos = Vec3::new(0.0, 0.0, 0.0);
    let trimmed = trim_behind_staging(&r, pos, std::f32::consts::FRAC_PI_2);
    let xs: Vec<f32> = trimmed.points.iter().map(|p| p.position.x).collect();
    assert_eq!(xs, vec![0.0, -50.0, -100.0], "row 1 is behind the line");
    assert_eq!(
        trimmed.start_heading_deg(),
        r.start_heading_deg(),
        "the staging row keeps its authored heading"
    );

    // Facing +X nothing is behind: the route comes back unchanged.
    let kept = trim_behind_staging(&r, pos, -std::f32::consts::FRAC_PI_2);
    assert_eq!(kept.points.len(), r.points.len());
}

// ---------------------------------------------------------------------------
// Production-path session tests
// ---------------------------------------------------------------------------

/// The authored roster spawns real AI participants: distinct `PlayerId`s,
/// `PlayerControl::Ai`, session ownership, `RaceProgress` and their own
/// vehicle configs — each its own base tune, the `_opp` file ignored as
/// the original ignores it (RACE-13).
#[test]
fn roster_spawns_distinct_ai_participants() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    assert_eq!(phase(&app), SessionPhase::Countdown);
    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 2, "both authored slots spawned");

    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let heavy = opponent_by_vehicle(&mut app, "vpheavy");

    for &e in &[vpt, heavy] {
        let player = app.world().get::<Player>(e).unwrap();
        assert_eq!(
            player.control,
            PlayerControl::Ai,
            "AI, never a network client"
        );
        assert!(app.world().get::<mm2_game::ObjectIdentity>(e).is_some());
        assert_eq!(
            app.world().get::<SessionEntity>(e).unwrap().0,
            1,
            "session-owned for teardown"
        );
        let progress = app.world().get::<RaceProgress>(e).unwrap();
        assert_eq!(progress.state, ParticipantState::AwaitingStart);
        assert!(
            app.world().get::<PlayerVehicle>(e).is_none(),
            "opponents are not the player vehicle"
        );
    }
    assert_ne!(
        app.world().get::<Player>(vpt).unwrap().id,
        app.world().get::<Player>(heavy).unwrap().id,
        "distinct player ids"
    );

    // Per-vehicle configs, not a cloned player car: each drives its own
    // base tune — `vpt` 1000 with its `_opp` file (2000) unread,
    // `vpheavy` 3000.
    let vpt_mass = app.world().get::<Vehicle>(vpt).unwrap().config.mass;
    let heavy_mass = app.world().get::<Vehicle>(heavy).unwrap().config.mass;
    assert_eq!(vpt_mass, 1000.0, "the _opp tune is never read");
    assert_eq!(heavy_mass, 3000.0, "base tune");

    // Route-row0 anchors: the authored .opp first points.
    let p = app.world().get::<Position>(vpt).unwrap().0;
    assert!(
        (p.x - 70.0).abs() < 2.0 && (p.z - 140.0).abs() < 2.0,
        "vpt at its route anchor: {p:?}"
    );
    let p = app.world().get::<Position>(heavy).unwrap().0;
    assert!(
        (p.x - 70.0).abs() < 2.0 && (p.z - 146.0).abs() < 2.0,
        "vpheavy at its route anchor: {p:?}"
    );
}

/// MP-4 (documented): a networked race fields no AI opponents — the
/// lobby's humans take the grid slots instead. The same authored
/// roster that spawns two drivers under `Local` authority spawns none
/// under either network authority — and a per-process unreplicated AI
/// set would diverge anyway, so the gate is also the only consistent
/// behavior until a slice replicates opponents.
#[test]
fn a_networked_event_spawns_no_ai_opponents() {
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let tmp = roster_install("", &[]);
        let config = SessionConfig {
            authority,
            ..event_config()
        };
        let mut app = event_app(config, vfs_of(tmp.path()));
        app.update();

        assert_eq!(phase(&app), SessionPhase::Countdown, "{authority:?}");
        assert!(
            opponents(&mut app).is_empty(),
            "{authority:?}: MP-4 — humans replace the authored roster"
        );
        // The local participant still loads onto the grid.
        let mut q = app
            .world_mut()
            .query_filtered::<&Player, With<PlayerVehicle>>();
        let player = q.single(app.world()).expect("the local car");
        assert_eq!(player.control, PlayerControl::Local);
    }
}

/// The authored staging heading reaches the real spawn: `vpt`'s row-0
/// `brake` of 90° faces −X even though its route legs run +X — the
/// authored facing wins, and every +X anchor is behind the line.
#[test]
fn authored_staging_heading_faces_the_spawn() {
    let mut staged = OPP_HEADER.to_string();
    staged.push_str("70,0,140,90,0,0,0,0,0\n");
    for x in [110.0, 140.0, 165.0, 180.0] {
        staged.push_str(&format!("{x},0,140,0,0,0,0,0,0\n"));
    }
    let tmp = roster_install("", &[("race0-a-0.opp", staged)]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let fwd = app.world().get::<Transform>(vpt).unwrap().rotation * Vec3::NEG_Z;
    assert!(fwd.x < -0.98, "authored 90° staging faces −X, fwd={fwd:?}");
    assert_eq!(
        app.world().get::<OpponentDriver>(vpt).unwrap().next,
        5,
        "every +X anchor sits behind the staged facing"
    );
}

/// The countdown's input lock holds opponents still — `VehicleInput`
/// stays zeroed until the race releases.
#[test]
fn countdown_locks_opponent_input() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    run(&mut app, 60); // well inside the ~180-update countdown

    for e in opponents(&mut app) {
        let input = app.world().get::<VehicleInput>(e).unwrap();
        assert_eq!(
            (input.throttle, input.brake, input.steering, input.handbrake),
            (0.0, 0.0, 0.0, 0.0),
            "countdown must hold opponent input"
        );
    }
}

/// Opponents drive their authored `.opp` routes through real physics and
/// earn progress through the shared `advance_race` path: both clear the
/// course's gates, cross the finish and record exactly one result each
/// in the shared ledger — while the undriven player never starts.
#[test]
fn opponents_drive_routes_and_finish() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    let vpt_id = app.world().get::<Player>(vpt).unwrap().id;
    let heavy_id = app.world().get::<Player>(heavy).unwrap().id;
    let start = app.world().get::<Position>(vpt).unwrap().0;

    run(&mut app, 1500); // countdown + ~90 m at up to 30 m/s + slack

    for (e, id) in [(vpt, vpt_id), (heavy, heavy_id)] {
        let progress = app.world().get::<RaceProgress>(e).unwrap();
        assert!(
            matches!(progress.state, ParticipantState::Finished { .. }),
            "opponent finished through the shared validation: {:?} cleared {}/3",
            progress.state,
            progress.cleared_count(),
        );
        assert!(
            app.world()
                .resource::<ResultLedger>()
                .iter()
                .any(|r| r.id.participant == id),
            "one ledger result for the AI participant"
        );
    }
    let pos = app.world().get::<Position>(vpt).unwrap().0;
    assert!(
        pos.x - start.x > 80.0,
        "physically drove the route: {start:?} → {pos:?}"
    );

    // The undriven local player is still unresolved — the AI's finish
    // ended nothing for them, and the race is still running.
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    assert_eq!(
        app.world().get::<RaceProgress>(car).unwrap().state,
        ParticipantState::Racing
    );
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        mm2_game::RacePhase::Running
    );
}

/// DSN-11: opponents race on behind the results screen. Once the
/// session reaches `Results` — the local driver resolved — the AI keeps
/// driving its route and still earns its finish through `advance_race`,
/// so the results list fills in instead of reading "still racing".
#[test]
fn opponents_race_on_behind_the_results_screen() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    while phase(&app) != SessionPhase::Playing {
        app.update();
    }
    // Stand in for the local driver's finish: the session leaves
    // `Playing` before any opponent has cleared a gate.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Results)
        .unwrap();

    run(&mut app, 1500);

    assert_eq!(phase(&app), SessionPhase::Results);
    for e in [vpt, heavy] {
        let progress = app.world().get::<RaceProgress>(e).unwrap();
        assert!(
            matches!(progress.state, ParticipantState::Finished { .. }),
            "opponent finished behind the results screen: {:?} cleared {}/3",
            progress.state,
            progress.cleared_count(),
        );
    }
}

/// An entry whose vehicle fails to load keeps its authored slot in the
/// roster but spawns nothing — the rest of the lineup still races.
#[test]
fn unloadable_vehicle_skips_only_its_slot() {
    let tmp = roster_install("vpmissing race0-a-0.opp 0.7 0 50 0.7 0 0 0 0 0 1.0\n", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 2, "the missing vehicle's slot spawns nothing");
    assert!(
        app.world_mut()
            .query_filtered::<&OpponentDriver, ()>()
            .iter(app.world())
            .all(|d| d.spec.vehicle != "vpmissing")
    );
}

/// An entry with a dead `.opp` reference keeps its slot and spawns (the
/// vehicle loads) but drives nothing — `route: None` means hold still.
#[test]
fn dead_route_reference_spawns_but_holds_still() {
    // `race0-a-9.opp` is never written → UnresolvedRoute on the roster.
    let tmp = roster_install("vpt race0-a-9.opp 0.7 0 50 0.7 0 0 0 0 0 1.0\n", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 3, "dead-route entry still spawns its vehicle");
    let dead = app
        .world_mut()
        .query_filtered::<(Entity, &OpponentDriver), ()>()
        .iter(app.world())
        .find(|(_, d)| d.spec.route.is_none())
        .map(|(e, _)| e)
        .expect("one entry carries no route");
    let start = app.world().get::<Position>(dead).unwrap().0;

    run(&mut app, 400); // past the countdown, driving

    let input = app.world().get::<VehicleInput>(dead).unwrap();
    assert_eq!(
        (input.throttle, input.brake, input.steering),
        (0.0, 0.0, 0.0),
        "no route → no drive"
    );
    let pos = app.world().get::<Position>(dead).unwrap().0;
    assert!(
        (pos - start).length() < 5.0,
        "route-less opponent holds still: {start:?} → {pos:?}"
    );
}

/// Restart despawns the whole opponent lineup with the session and
/// rebuilds it — no stale AI entities, controllers or progress survive.
#[test]
fn restart_respawns_the_lineup() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    run(&mut app, 5);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == SessionPhase::Countdown
            && app.world().resource::<Session>().generation() == 2
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never returned to Countdown");

    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 2, "lineup respawned with the new session");
    for e in opps {
        assert_eq!(
            app.world().get::<SessionEntity>(e).unwrap().0,
            2,
            "fresh generation owns the respawned opponents"
        );
        assert_eq!(
            app.world().get::<RaceProgress>(e).unwrap().state,
            ParticipantState::AwaitingStart
        );
        assert_eq!(
            app.world().get::<OpponentDriver>(e).unwrap().next,
            1,
            "the reached row-0 anchor is skipped — the chase starts on the first leg"
        );
    }
}

/// F14-AC05's Ordered leg: a mid-race restart on a lapped, rostered
/// circuit restores grid, counters, clocks and event objects exactly
/// once — banked lap/gate/route credit clears, the lineup respawns on
/// its authored slots, the race clock and each `RouteGateLine`'s
/// high-water mark reset with the generation, the markers restamp and
/// the ledger stays generation-scoped.
#[test]
fn restart_restores_the_circuit_grid_counters_and_objects() {
    let tmp = circuit_install();
    let mut app = event_app(circuit_config(), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    // Spawn-time chase indices — the grid state the restart must
    // restore (keyed by roster index, not entity: teardown mints new).
    let mut initial_next = std::collections::BTreeMap::new();
    for e in opponents(&mut app) {
        let d = app.world().get::<OpponentDriver>(e).unwrap();
        initial_next.insert(d.index, d.next);
    }

    // Let the field bank real progress — gates cleared, laps run,
    // route arc grown, the race clock ticking.
    run(&mut app, 1600);
    assert!(
        matches!(
            app.world().resource::<RaceState>().phase,
            mm2_game::RacePhase::Running
        ),
        "the restart must cut a live race"
    );
    let banked: Vec<(u32, usize, u32, u32)> = opponents(&mut app)
        .into_iter()
        .map(|e| {
            let p = app.world().get::<RaceProgress>(e).unwrap();
            (p.lap, p.cleared_count(), p.crossings, p.route_clears)
        })
        .collect();
    assert!(
        banked
            .iter()
            .any(|(lap, cleared, x, d)| *lap > 0 || *cleared > 0 || *x > 0 || *d > 0),
        "generation 1 banked no progress to restore: {banked:?}"
    );

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == SessionPhase::Countdown
            && app.world().resource::<Session>().generation() == 2
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never returned to Countdown");

    // The race resource is the new session's: countdown from full,
    // clock at zero, generation 2.
    let race = app.world().resource::<RaceState>();
    assert_eq!(race.generation, 2, "race belongs to the new session");
    assert!(matches!(race.phase, mm2_game::RacePhase::Countdown { .. }));
    assert_eq!(race.clock, 0, "the old race clock must not survive");
    let gates = race.definition.checkpoints.len();

    // The grid: same lineup once, fresh ownership, authored slots,
    // zeroed counters, the route bind re-anchored at the spawn.
    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 2, "lineup respawned exactly once");
    for e in opps {
        assert_eq!(
            app.world().get::<SessionEntity>(e).unwrap().0,
            2,
            "fresh generation owns the respawned opponents"
        );
        let p = app.world().get::<RaceProgress>(e).unwrap();
        assert!(matches!(p.state, ParticipantState::AwaitingStart));
        assert_eq!(
            (
                p.lap,
                p.next,
                p.cleared_count(),
                p.crossings,
                p.route_clears
            ),
            (0, 0, 0, 0, 0),
            "banked lap/gate/route credit must reset: {p:?}"
        );
        let d = app.world().get::<OpponentDriver>(e).unwrap();
        assert_eq!(
            d.next, initial_next[&d.index],
            "the chase index restored to its spawn value"
        );
        let pos = app.world().get::<Position>(e).unwrap().0;
        let staged = d.spec.route.as_ref().expect("authored route").points[0].position;
        assert!(
            (pos.x - staged.x).abs() < 3.0 && (pos.z - staged.z).abs() < 3.0,
            "opponent {} back on its route's staging point, got {pos:?}",
            d.index
        );
        let line = app
            .world()
            .get::<RouteGateLine>(e)
            .expect("an Ordered roster entry carries the route bind");
        let neural = d.neural.as_ref().expect("a routed entry has a driver");
        assert_eq!(neural.driven, 0.0, "nothing driven yet");
        assert_eq!(
            line.arc_high,
            neural.progress(),
            "the route-credit high-water rebinds at the grid, not mid-course"
        );
    }

    // The player sits back on authored slot 0 — the whole grid is
    // restored, not just the AI half.
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos.x - 60.0).abs() < 3.0 && (pos.z - COURSE_Z).abs() < 3.0,
        "player back on the authored start line, got {pos:?}"
    );

    // Event objects restamped once: one marker per Ordered gate, no
    // finish trigger, nothing from generation 1 left behind.
    let markers = app
        .world_mut()
        .query_filtered::<Entity, With<race::CheckpointMarker>>()
        .iter(app.world())
        .count();
    assert_eq!(markers, gates, "gate markers respawned exactly once");

    // The ledger keeps generation-1 results — they just do not belong
    // to this race.
    let ledger = app.world().resource::<ResultLedger>();
    assert!(
        ledger.standings_in(2).is_empty(),
        "a stale result must not rank into generation 2"
    );
}

/// F17-A.7's session leg (RACE-3's Circuit parenthetical): a launched
/// `SessionCustomization::race` applies after the authored setup
/// builds — the effective definition takes the picked lap count and
/// the roster shrinks to the picked prefix before opponents spawn.
#[test]
fn circuit_race_picks_apply_to_the_session_definition_and_roster() {
    let tmp = circuit_install();
    let config = SessionConfig {
        customization: Some(SessionCustomization {
            conditions: SessionConditions::default(),
            densities: Densities::DEFAULT,
            race: Some(RaceCustomization {
                laps: 5,
                opponents: 1,
            }),
        }),
        ..circuit_config()
    };
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let race = app.world().resource::<RaceState>();
    assert_eq!(race.definition.laps, 5, "the picked lap count bound");
    assert_eq!(
        race.definition.params.opponents, 1,
        "the param tracks the applied count"
    );
    let opps = opponents(&mut app);
    assert_eq!(opps.len(), 1, "the picked prefix spawns");
    let d = app.world().get::<OpponentDriver>(opps[0]).unwrap();
    assert_eq!(
        d.spec.vehicle, "vpt",
        "the kept entry is the authored first"
    );
}

/// The same picks never fabricate roster entries — a count beyond the
/// authored `[Opponent]` block clamps to the wired lineup.
#[test]
fn circuit_race_picks_never_exceed_the_wired_roster() {
    let tmp = circuit_install();
    let config = SessionConfig {
        customization: Some(SessionCustomization {
            conditions: SessionConditions::default(),
            densities: Densities::DEFAULT,
            race: Some(RaceCustomization {
                laps: 3,
                opponents: 9,
            }),
        }),
        ..circuit_config()
    };
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let race = app.world().resource::<RaceState>();
    assert_eq!(race.definition.laps, 3);
    assert_eq!(
        race.definition.params.opponents, 2,
        "clamped to the aimap's rows"
    );
    assert_eq!(opponents(&mut app).len(), 2, "no opponent is fabricated");
}

pub(crate) fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

// ---------------------------------------------------------------------------
// F15-B.1 — traffic avoidance / overtake
// ---------------------------------------------------------------------------

/// The odometer behind `opp_drv=` counts driven XZ distance and
/// driving time, and a teleport-sized jump adds time but no distance —
/// a re-anchor is the assist, not driving.
#[test]
fn drive_stats_count_driving_but_not_jumps() {
    let mut s = DriveStats::default();
    s.record(Vec3::new(0.0, 0.0, 0.0), 0.5);
    s.record(Vec3::new(1.5, 9.0, 2.0), 0.5);
    assert!((s.distance - 2.5).abs() < 1e-4, "XZ only: {}", s.distance);
    s.record(Vec3::new(100.0, 0.0, 2.0), 0.5);
    assert!((s.distance - 2.5).abs() < 1e-4, "jump excluded");
    assert!((s.seconds - 1.5).abs() < 1e-4);
}

/// Regression for the review's ambiguous-pick finding
/// (`update_checkpoint_markers`): the markers show the *local* driver's
/// view. Opponents carry `Player` too, so the system cannot take the
/// first `RaceProgress` it meets — moving the local player into an
/// archetype created after the AI's makes that order-dependent pick
/// land on an opponent.
#[test]
fn checkpoint_markers_track_the_local_participant() {
    #[derive(Component)]
    struct Probe;

    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    app.world_mut().entity_mut(car).insert(Probe);

    // An AI clears gate 0 through the contract — `advance` anchors the
    // segment, then sweeps through the trigger — while the local
    // driver clears nothing.
    let def = app.world().resource::<RaceState>().definition.clone();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    {
        let mut p = app.world_mut().get_mut::<RaceProgress>(vpt).unwrap();
        p.state = ParticipantState::Racing;
        p.advance(&def, Vec3::new(85.0, 0.0, COURSE_Z));
        p.advance(&def, Vec3::new(135.0, 0.0, COURSE_Z));
        assert!(p.is_cleared(0), "the AI really swept gate 0");
    }
    app.update();

    let vis = app
        .world_mut()
        .query_filtered::<(&race::CheckpointMarker, &Visibility), ()>()
        .iter(app.world())
        .find(|(m, _)| m.gate == Some(0))
        .map(|(_, v)| *v);
    assert_eq!(
        vis,
        Some(Visibility::Visible),
        "an AI's cleared gate must not hide the local driver's marker"
    );
}

// ---------------------------------------------------------------------------
// F15-B.2 — authored parameter tail
// ---------------------------------------------------------------------------

/// The authored tail reaches the driver through the production roster
/// path: `maxThrottle` is the ceiling on the driver's throttle. The
/// other columns parametrised the original's own controller — its
/// corner speed, braking threshold, look-ahead and what it swept its
/// path against — and have no counterpart in a learned driver; they
/// stay decoded on the spec, verbatim.
#[test]
fn authored_tail_binds_the_drivers_throttle() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.90 0 50.0 0.4 1 1 0 1 0 1.5\nvpheavy race0-a-1.opp 0.80 0 120.0 0.7 1 1 1 0 0 2.0\n",
        &[],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let d = app.world().get::<OpponentDriver>(vpt).unwrap();
    assert!((d.throttle_cap - 0.9).abs() < 1e-6);
    let params = d.spec.drive_params();
    assert_eq!(params.corner_brake, Some(0.4));
    assert_eq!(params.avoid_players, Some(false));
    assert_eq!(params.distance_padding, Some(50.0));
    assert_eq!(params.corner_speed_multiplier, Some(1.5));

    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    let d = app.world().get::<OpponentDriver>(heavy).unwrap();
    assert!((d.throttle_cap - 0.8).abs() < 1e-6);
    assert_eq!(d.spec.drive_params().avoid_opponents, Some(false));
}

/// The same car on the same lane shape differs only in authored
/// `maxThrottle` (0.5 vs 1.0): through the production path the
/// uncapped driver covers measurably more road at a fixed tick — the
/// authored difficulty dial is observable, not just parsed.
#[test]
fn authored_max_throttle_measures_on_track() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.5 0 50.0 0.7 1 1 1 1 0 1.0\nvpt race0-a-1.opp 1.0 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    run(&mut app, 480);

    let mut by_cap: Vec<(f32, f32)> = app
        .world_mut()
        .query::<(&OpponentDriver, &Position)>()
        .iter(app.world())
        .map(|(d, p)| (d.throttle_cap, p.0.x))
        .collect();
    by_cap.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (slow_x, fast_x) = (by_cap[0].1, by_cap[1].1);
    assert!(
        fast_x - slow_x > 10.0,
        "authored maxThrottle must measure: 0.5-cap x={slow_x:.1} vs 1.0-cap x={fast_x:.1}"
    );
}

// ---------------------------------------------------------------------------
// F15-B.3 — bounded re-anchor recovery
// ---------------------------------------------------------------------------

/// A mid-leg projection steps back `REANCHOR_BACK` along the chased
/// leg and faces down-leg.
#[test]
fn reanchor_pose_projects_onto_the_chased_leg() {
    let r = route(&[[70.0, 0.0, 140.0], [140.0, 0.0, 140.0], [165.0, 0.0, 140.0]]);
    // Chasing anchor 2 → the chased leg is 140→165; pos projects at
    // x=150 and steps back 4 m to x=146 on the route line.
    let (pose, yaw) = reanchor_pose(&r, 2, Vec3::new(150.0, 0.0, 150.0), 0.0, |_| false);
    assert!(
        (pose.x - 146.0).abs() < 1e-4 && (pose.z - 140.0).abs() < 1e-4,
        "{pose:?}"
    );
    assert!(
        (yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "the +X leg faces vehicle yaw −π/2, got {yaw}"
    );
}

/// A candidate landing inside an un-cleared trigger keeps walking back
/// along the polyline — across a leg boundary — until it stands clear,
/// so the assist cannot bank a gate the car never drove (AC04).
#[test]
fn reanchor_pose_walks_back_out_of_uncleared_triggers() {
    let r = route(&[
        [70.0, 0.0, 140.0],
        [110.0, 0.0, 140.0],
        [140.0, 0.0, 140.0],
        [165.0, 0.0, 140.0],
    ]);
    // Triggers at x=110 and x=140 (r=15) are still pending; x=165 is
    // already cleared so it must not force the walk further back.
    let blocked = |p: Vec3| {
        [(110.0, 15.0), (140.0, 15.0)]
            .iter()
            .any(|(cx, rad)| (p.x - cx).abs() < *rad && (p.z - 140.0).abs() < *rad)
    };
    // next=3 → chased leg 140→165; pos x=135 projects behind the leg
    // start, so the walk crosses onto leg 110→140 and keeps going
    // while either pending cylinder covers the candidate.
    let (pose, _) = reanchor_pose(&r, 3, Vec3::new(135.0, 0.0, 140.0), 0.0, blocked);
    assert!(
        (pose.x - 92.0).abs() < 1e-3 && (pose.z - 140.0).abs() < 1e-3,
        "walked back out of both pending triggers: {pose:?}"
    );
    assert!(!blocked(pose));

    // With 110 already cleared the same stuck spot only steps out of
    // the 140 cylinder — the walk stops at x=124, not behind 110.
    let cleared_110 = |p: Vec3| (p.x - 140.0).abs() < 15.0 && (p.z - 140.0).abs() < 15.0;
    let (pose, _) = reanchor_pose(&r, 3, Vec3::new(135.0, 0.0, 140.0), 0.0, cleared_110);
    assert!(
        (pose.x - 124.0).abs() < 1e-3,
        "a cleared gate does not push the landing back: {pose:?}"
    );
}

/// A closed route's `next == 0` walks the wrap leg (last → first), and
/// an open route cannot walk back before its first point.
#[test]
fn reanchor_pose_wraps_closed_and_clamps_open() {
    // Closed square with a 10 m closing gap (≤ ROUTE_LOOP).
    let r = route(&[
        [0.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [100.0, 0.0, 100.0],
        [0.0, 0.0, 100.0],
        [0.0, 0.0, 10.0],
    ]);
    // next=0 chasing point 0 → the chased leg is p4 (0,10) → p0 (0,0);
    // a car beside it projects mid-leg (z=5) and steps back toward p4.
    let (pose, yaw) = reanchor_pose(&r, 0, Vec3::new(5.0, 0.0, 5.0), 0.0, |_| false);
    assert!(
        (pose.z - 9.0).abs() < 1e-4 && pose.x.abs() < 1e-4,
        "on the wrap leg, {pose:?}"
    );
    assert!(yaw.abs() < 1e-4, "the −Z wrap leg keeps yaw 0, got {yaw}");

    let (landing, _, next) =
        reanchor_pose_with_progress(&r, 0, Vec3::new(5.0, 0.0, 1.0), 0.0, |_| false);
    assert_eq!(next, 0, "the closing leg still targets the first point");
    assert!((landing.z - 5.0).abs() < 1e-4);
    let (landing, _, next) =
        reanchor_pose_with_progress(&r, 1, Vec3::new(1.0, 0.0, 0.0), 0.0, |_| false);
    assert_eq!(
        next, 0,
        "walking across the lap boundary retains the selected closing leg"
    );
    assert!((landing.z - 3.0).abs() < 1e-4);

    // Open route (60 m leg > ROUTE_LOOP, so not closed), car behind
    // the first anchor chasing it (next=0): leg 0 is the approach
    // line, and the walk clamps at its start.
    let open = route(&[[70.0, 0.0, 140.0], [130.0, 0.0, 140.0]]);
    let (pose, _) = reanchor_pose(&open, 0, Vec3::new(60.0, 0.0, 160.0), 0.0, |_| true);
    assert_eq!(pose, Vec3::new(70.0, 0.0, 140.0), "bounded at route start");
}

/// Degenerate routes anchor at what they have; an empty route cannot
/// re-anchor at all.
#[test]
fn reanchor_pose_handles_degenerate_routes() {
    let single = route(&[[50.0, 2.0, 60.0]]);
    let (pose, yaw) = reanchor_pose(&single, 0, Vec3::new(9.0, 0.0, 9.0), 0.7, |_| false);
    assert_eq!(pose, Vec3::new(50.0, 2.0, 60.0));
    assert_eq!(yaw, 0.7, "no leg → the car's own yaw");

    let empty = OpponentRoute { points: vec![] };
    let (pose, yaw) = reanchor_pose(&empty, 0, Vec3::new(9.0, 1.0, 9.0), 0.7, |_| false);
    assert_eq!((pose, yaw), (Vec3::new(9.0, 1.0, 9.0), 0.7));
}

/// A closed route whose anchors all sit on one XZ point has only
/// zero-length legs: `walked` can never reach `REANCHOR_WALK` and the
/// `!closed` escape is off, so before the step cap the walk cycled the
/// legs forever (authored-numbers audit finding 1). The roster rejects
/// the shape at distillation; the walk still bounds it — and keeps
/// bounding it when every landing is `blocked` too.
#[test]
fn reanchor_pose_bounds_a_collapsed_closed_route() {
    let collapsed = route(&[
        [50.0, 0.0, 60.0],
        [50.0, 1.5, 60.0],
        [50.0, 3.0, 60.0],
        [50.0, 0.0, 60.0],
    ]);
    assert!(
        !collapsed.drivable(),
        "a vertical stack carries no driveable line"
    );
    let (pose, _) = reanchor_pose(&collapsed, 0, Vec3::new(50.0, 0.0, 60.0), 0.7, |_| true);
    assert!(pose.is_finite(), "bounded landing: {pose:?}");
    let (pose, _) = reanchor_pose(&collapsed, 2, Vec3::new(50.0, 0.5, 60.0), 0.7, |_| false);
    assert!(pose.is_finite(), "bounded landing: {pose:?}");
}

/// Non-finite anchors make every leg-length comparison false — the
/// same never-progress shape as the collapsed route, through the
/// `blocked` retry loop this time. The walk terminates and never
/// hands back a non-finite pose; a non-finite input pose returns
/// verbatim like the empty route does.
#[test]
fn reanchor_pose_bounds_a_nonfinite_route() {
    let mut r = route(&[[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [50.0, 0.0, 50.0]]);
    r.points[1].position = Vec3::new(f32::NAN, 0.0, 0.0);
    assert!(!r.drivable(), "a NaN anchor poisons the line");
    let (pose, _) = reanchor_pose(&r, 2, Vec3::new(50.0, 0.0, 10.0), 0.0, |_| true);
    assert!(pose.is_finite(), "{pose:?}");

    let nan = Vec3::new(f32::NAN, 0.0, 0.0);
    let (pose, yaw) = reanchor_pose(&r, 0, nan, 0.3, |_| false);
    assert!(pose.x.is_nan() && yaw == 0.3, "the input stands: {pose:?}");
}

/// The penned-opponent bound end to end (F15-B.3, AC03): `vpt` is
/// teleported into a walled pocket off its lane where no escape can
/// progress — the displacement window spends its budget and the
/// disclosed `ResetVehicle` re-anchor drops it back on the chased
/// route leg, walked clear of the gates it has not cleared. The jump
/// banks nothing (`Teleported` broke the segment and the landing is
/// outside every pending trigger) and the car resumes driving to a
/// real finish.
#[test]
fn permanently_stuck_opponent_reanchors_and_resumes() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");

    // During the countdown, wall off a pocket off the lane at
    // (134,·,180) — beyond every gate's z reach — and move the car in
    // through the production teleport contract (`Teleported` breaks
    // the swept segment, so even this test jump cannot bank a gate).
    let y = app.world().get::<Position>(vpt).unwrap().0.y;
    for (center, size) in [
        (Vec3::new(132.0, y + 1.0, 180.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(136.0, y + 1.0, 180.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(134.0, y + 1.0, 178.0), Vec3::new(8.0, 3.0, 0.4)),
        (Vec3::new(134.0, y + 1.0, 182.0), Vec3::new(8.0, 3.0, 0.4)),
    ] {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Transform::from_translation(center),
        ));
    }
    app.world_mut().get_mut::<Position>(vpt).unwrap().0 = Vec3::new(134.0, y, 180.0);
    app.world_mut()
        .get_mut::<Transform>(vpt)
        .unwrap()
        .translation = Vec3::new(134.0, y, 180.0);
    app.world_mut()
        .entity_mut(vpt)
        .insert(mm2_vehicle::Teleported);

    // The car sits penned through release and every failed escape;
    // the pocket never lets it near a pending trigger, so cleared
    // stays 0 until the re-anchor fires.
    let mut fired_at = None;
    for u in 0..1400 {
        app.update();
        let d = app.world().get::<OpponentDriver>(vpt).unwrap();
        if d.reanchors > 0 {
            assert!(
                d.stuck_peak >= REANCHOR_FRAMES,
                "the spent window is the recorded stuck duration: {}",
                d.stuck_peak
            );
            let backed = d.neural.as_ref().map_or(0, |n| n.unstick.count);
            assert!(
                backed >= 1,
                "the penned car counted its failed back-ups: {backed}"
            );
            fired_at = Some(u);
            break;
        }
        if u > 200 {
            assert_eq!(
                app.world()
                    .get::<RaceProgress>(vpt)
                    .unwrap()
                    .cleared_count(),
                0,
                "the penned car banked a gate at update {u}"
            );
        }
    }
    assert!(
        fired_at.is_some(),
        "the bounded re-anchor never fired for a penned car"
    );

    // The teleport itself may land an update after the counter flips
    // (message ordering) and the marker is consumed at the next
    // frame's FixedLast — give the pipeline its two frames.
    run(&mut app, 3);

    // The jump landed back on the chased leg, walked clear of the
    // pending gates — inside none of them, so the next step's crossing
    // is the car's own driving, not the teleport's.
    let pose = app.world().get::<Position>(vpt).unwrap().0;
    // Back where its guide last had it — it never drove a metre of the
    // route — not abreast of the pocket it was put in.
    assert!(
        (pose.z - COURSE_Z).abs() < 2.0 && pose.x > 60.0 && pose.x < 90.0,
        "re-anchored onto the route where it left it: {pose:?}"
    );
    let rot = app.world().get::<Rotation>(vpt).unwrap().0;
    assert!((rot * Vec3::Y).y > 0.99, "re-anchor lands upright: {rot:?}");
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(vpt).is_none(),
        "reanchor_teleported_participants consumed the marker"
    );
    assert_eq!(
        app.world()
            .get::<RaceProgress>(vpt)
            .unwrap()
            .cleared_count(),
        0,
        "the teleport itself banked nothing"
    );

    // And it resumes: the re-anchored car re-drives the course and
    // finishes through the shared validation.
    run(&mut app, 900);
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "the re-anchored opponent finishes for real: {:?} cleared {}/3",
        progress.state,
        progress.cleared_count()
    );
}

/// The dispatch mechanics without the 900-frame wait: a driver whose
/// stuck budget is spent teleports through `ResetVehicle` on the next
/// updates — `reanchors` records it and the pass state clears.
#[test]
fn reanchor_dispatches_through_the_production_reset_path() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    run(&mut app, 260); // released and driving

    let before = app.world().get::<Position>(vpt).unwrap().0;
    {
        let mut d = app.world_mut().get_mut::<OpponentDriver>(vpt).unwrap();
        d.stuck_pos = before;
        d.stuck_frames = REANCHOR_FRAMES - 1; // one driving update short of the bound
    }
    run(&mut app, 4);

    let d = app.world().get::<OpponentDriver>(vpt).unwrap();
    assert_eq!(d.reanchors, 1, "the spent budget fired once");
    // The window restarted at the landing pose — a fresh small count
    // while the car accelerates away, not the spent budget.
    assert!(d.stuck_frames < 60, "stuck_frames={}", d.stuck_frames);
    // The peak survives the reset — the session's longest stuck spell
    // stays on the record even though the live window re-anchored.
    assert!(
        d.stuck_peak >= REANCHOR_FRAMES,
        "stuck_peak={}",
        d.stuck_peak
    );
    let after = app.world().get::<Position>(vpt).unwrap().0;
    assert!(
        (after.z - COURSE_Z).abs() < 2.0 && after.x < before.x + 4.0,
        "back on the route line, not ahead of the failure: {before:?} → {after:?}"
    );
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(vpt).is_none(),
        "the swept-segment break was consumed"
    );
}

/// Drive the dispatch to a re-anchor with `extra` spawned behind the
/// stuck car, and return the landing pose and the car's own height.
fn reanchor_landing(extra: impl FnOnce(&mut App, Vec3)) -> Vec3 {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    run(&mut app, 260); // released and driving
    let before = app.world().get::<Position>(vpt).unwrap().0;
    extra(&mut app, before);
    {
        let mut d = app.world_mut().get_mut::<OpponentDriver>(vpt).unwrap();
        d.stuck_pos = before;
        d.stuck_frames = REANCHOR_FRAMES - 1;
    }
    // The landing is the pose of the reset, before the car settles.
    for _ in 0..8 {
        app.update();
        if app.world().get::<OpponentDriver>(vpt).unwrap().reanchors > 0 {
            break;
        }
    }
    assert_eq!(app.world().get::<OpponentDriver>(vpt).unwrap().reanchors, 1);
    app.world().get::<Position>(vpt).unwrap().0
}

/// The route's heights are samples, not a ground promise: the landing
/// is seated on the *static* ground under the level car — a curb or a
/// raised slab lifts it onto that surface, a loose body that happens to
/// lie behind the car (a crate, another car) does not.
#[test]
fn reanchor_seats_on_static_ground_not_on_loose_bodies() {
    let flat = reanchor_landing(|_, _| {});
    let behind = |before: Vec3| (before.x - 40.0, before.x - 3.0);
    let slab = |app: &mut App, before: Vec3, body: RigidBody, top: f32| {
        let (lo, hi) = behind(before);
        app.world_mut().spawn((
            body,
            Collider::cuboid(hi - lo, 1.0, 20.0),
            Transform::from_xyz((lo + hi) / 2.0, top - 0.5, before.z),
        ));
    };
    let raised = reanchor_landing(|app, before| slab(app, before, RigidBody::Static, 0.4));
    let loose = reanchor_landing(|app, before| slab(app, before, RigidBody::Dynamic, 1.0));
    assert!(
        (raised.y - (flat.y + 0.4)).abs() < 0.15,
        "static slab seats the car on its top: flat {flat:?} raised {raised:?}"
    );
    assert!(
        (loose.y - flat.y).abs() < 0.15,
        "a dynamic body is not ground: flat {flat:?} loose {loose:?}"
    );
    assert!(
        raised.x < flat.x + 0.5 && raised.x > flat.x - 0.5,
        "seating keeps the walked landing's x/z: {flat:?} → {raised:?}"
    );
}

/// A field that keeps making progress never triggers the assist:
/// both opponents drive the whole course and `reanchors` stays 0.
#[test]
fn progressing_opponents_never_reanchor() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    run(&mut app, 1200); // past the budget — both cars finish inside it

    let mut q = app.world_mut().query::<(Entity, &OpponentDriver)>();
    let counts: Vec<(Entity, u32, u32)> = q
        .iter(app.world())
        .map(|(e, d)| (e, d.reanchors, d.stuck_peak))
        .collect();
    assert!(!counts.is_empty());
    for (e, n, peak) in counts {
        assert_eq!(n, 0, "opponent {e:?} re-anchored while progressing");
        // The stuck measure is live — a moving car's bubble re-seats
        // every REANCHOR_DIST of travel, so its peak is a small
        // positive count, nowhere near the bound.
        assert!(peak > 0, "opponent {e:?} never left a bubble reading");
        assert!(
            peak < REANCHOR_FRAMES,
            "opponent {e:?} peaked at {peak} while progressing"
        );
    }
}

// ---------------------------------------------------------------------------
// F15-B.9 — measured difficulty effects (AC06)
// ---------------------------------------------------------------------------

/// The session difficulty selects which authored lineup an event
/// fields — Amateur reads `<stem>.aimap`, Professional `<stem>.aimap_p`
/// (RACE-11) — through the production `load_session_world` →
/// `event_aimap` → `opponent_roster` → `load_opponent` path. The
/// variants here differ in roster order, vehicle assignment, tuning
/// tail and route geometry, so the pick is observable on the spawned
/// field, not just in the parsed records.
#[test]
fn session_difficulty_fields_the_authored_variant() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.55 0 50.0 0.7 1 1 1 1 0 1.0\nvpheavy race0-a-1.opp 0.60 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[
            (
                "race0.aimap_p",
                aimap_with_opponents(
                    "vpheavy race0-p-0.opp 1.00 0 50.0 0.7 1 1 1 1 0 1.0\nvpt race0-p-1.opp 0.95 0 50.0 0.7 1 1 1 1 0 1.0\n",
                ),
            ),
            (
                "race0-p-0.opp",
                opp_file(&[
                    [70.0, 0.0, 152.0],
                    [110.0, 0.0, 152.0],
                    [140.0, 0.0, 152.0],
                    [165.0, 0.0, 152.0],
                    [180.0, 0.0, 152.0],
                ]),
            ),
            (
                "race0-p-1.opp",
                opp_file(&[
                    [70.0, 0.0, 158.0],
                    [110.0, 0.0, 158.0],
                    [140.0, 0.0, 158.0],
                    [165.0, 0.0, 158.0],
                    [180.0, 0.0, 158.0],
                ]),
            ),
        ],
    );

    // (roster slot, vehicle, bound throttle cap, first authored route
    // point's z — the lane offset proves which `.opp` variant bound).
    let lineup = |difficulty: Difficulty| {
        let mut app = event_app(
            SessionConfig {
                difficulty,
                ..event_config()
            },
            vfs_of(tmp.path()),
        );
        app.update();
        let mut rows: Vec<(usize, String, f32, f32)> = app
            .world_mut()
            .query::<&OpponentDriver>()
            .iter(app.world())
            .map(|d| {
                (
                    d.index,
                    d.spec.vehicle.clone(),
                    d.throttle_cap,
                    d.spec.route.as_ref().unwrap().points[0].position.z,
                )
            })
            .collect();
        rows.sort_by_key(|r| r.0);
        rows
    };

    assert_eq!(
        lineup(Difficulty::Amateur),
        vec![
            (0, "vpt".to_string(), 0.55, 140.0),
            (1, "vpheavy".to_string(), 0.60, 146.0),
        ],
        "amateur binds <stem>.aimap — its own vehicles, tails and -a- routes"
    );
    assert_eq!(
        lineup(Difficulty::Professional),
        vec![
            (0, "vpheavy".to_string(), 1.0, 152.0),
            (1, "vpt".to_string(), 0.95, 158.0),
        ],
        "professional binds <stem>.aimap_p — a different authored field"
    );
}

/// AC06's measured-effects leg: the same car chases the same lane
/// geometry in both sessions, and the only difference is which aimap
/// variant the session difficulty selected — amateur's authored
/// `maxThrottle` 0.55 against professional's 1.00. The professional
/// field covers measurably more road at a fixed tick: the difficulty
/// switch is a measured change through the production drive path, not
/// a label. Both runs read `catch_up == 0` — a lone opponent leads the
/// parked player, so the delta is authored tuning, never the
/// disclosed assist (DSN-27).
#[test]
fn session_difficulty_measures_on_track() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.55 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[
            (
                "race0.aimap_p",
                aimap_with_opponents("vpt race0-p-0.opp 1.00 0 50.0 0.7 1 1 1 1 0 1.0\n"),
            ),
            (
                "race0-p-0.opp",
                opp_file(&[
                    [70.0, 0.0, 140.0],
                    [110.0, 0.0, 140.0],
                    [140.0, 0.0, 140.0],
                    [165.0, 0.0, 140.0],
                    [180.0, 0.0, 140.0],
                ]),
            ),
        ],
    );

    let driven = |difficulty: Difficulty| {
        let mut app = event_app(
            SessionConfig {
                difficulty,
                ..event_config()
            },
            vfs_of(tmp.path()),
        );
        app.update();
        let vpt = opponent_by_vehicle(&mut app, "vpt");
        run(&mut app, 480);
        app.world().get::<Position>(vpt).unwrap().0.x
    };

    let amateur_x = driven(Difficulty::Amateur);
    let pro_x = driven(Difficulty::Professional);
    assert!(
        pro_x - amateur_x > 10.0,
        "the difficulty switch must measure on track: \
         amateur x={amateur_x:.1} vs professional x={pro_x:.1}"
    );
}

// ---------------------------------------------------------------------------
// F15-B.10 — the spec's representative avoidance matrix, production path
// ---------------------------------------------------------------------------

/// Edge: an overturned opponent. A car flipped onto its roof cannot
/// drive, but it is not lost — `vehicle_self_right` (the authored
/// `assists.self_right_delay` window) flops it back onto its wheels in
/// place and the route chase resumes from there. In-place recovery,
/// not the disclosed teleport: `reanchors` stays 0, and the upended
/// spell banks nothing.
#[test]
fn an_overturned_opponent_self_rights_and_resumes() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");

    // Roll the car onto its roof during the countdown — in place, so
    // the flip itself sweeps no gate — and stop it dead.
    let flipped =
        app.world().get::<Rotation>(vpt).unwrap().0 * Quat::from_rotation_z(std::f32::consts::PI);
    assert!(
        (flipped * Vec3::Y).y < -0.5,
        "the flip really puts the roof down"
    );
    app.world_mut().get_mut::<Rotation>(vpt).unwrap().0 = flipped;
    app.world_mut().get_mut::<Transform>(vpt).unwrap().rotation = flipped;
    app.world_mut().get_mut::<LinearVelocity>(vpt).unwrap().0 = Vec3::ZERO;
    app.world_mut().get_mut::<AngularVelocity>(vpt).unwrap().0 = Vec3::ZERO;
    app.world_mut()
        .entity_mut(vpt)
        .insert(mm2_vehicle::Teleported);

    // The authored 2 s self-right delay must right it well inside the
    // 15 s re-anchor window — the in-place recovery gets its turn
    // first, and it happens even though the countdown may still hold.
    let mut righted_at = None;
    for u in 0..400 {
        app.update();
        let up = app.world().get::<Rotation>(vpt).unwrap().0 * Vec3::Y;
        if up.y > 0.9 {
            righted_at = Some(u);
            break;
        }
    }
    assert!(
        righted_at.is_some_and(|u| u < REANCHOR_FRAMES),
        "the upended car never self-righted"
    );
    assert_eq!(
        app.world().get::<OpponentDriver>(vpt).unwrap().reanchors,
        0,
        "self-righting is in-place — no disclosed teleport was needed"
    );
    assert_eq!(
        app.world()
            .get::<RaceProgress>(vpt)
            .unwrap()
            .cleared_count(),
        0,
        "the upended spell banked no gate"
    );

    // And the recovery is real: the car drives the route on and
    // finishes through the same swept-trigger validation.
    run(&mut app, 1000);
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "the righted opponent resumes and finishes: {:?} cleared {}/3",
        progress.state,
        progress.cleared_count()
    );
}

/// Edge: a junction miss. A car carried past a turn must rejoin the
/// polyline — `point_reached`'s perpendicular-plane rule may mark the
/// missed anchor passed (chase forward, never U-turn) or the chase may
/// bend back to it; either way the miss is a bounded rejoin, not a
/// dead-end. The route bends +z after x=110; the teleport drops the
/// car past the corner on the old line's extension. The one gate and
/// the finish live on the far leg, so any rejoin style sweeps them.
#[test]
fn a_junction_miss_rejoins_the_route() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n\
         vpheavy race0-a-1.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[
            (
                "race0-a-0.opp",
                opp_file(&[
                    [70.0, 0.0, 140.0],
                    [110.0, 0.0, 140.0],
                    [115.0, 0.0, 165.0],
                    [185.0, 0.0, 170.0],
                ]),
            ),
            (
                "race0waypoints.csv",
                format!(
                    "{WAYPOINTS}{}{}{}",
                    waypoint_row(60.0, COURSE_Z),
                    waypoint_row(170.0, 170.0),
                    waypoint_row(185.0, 170.0),
                ),
            ),
        ],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");

    run(&mut app, 280);
    let y = app.world().get::<Position>(vpt).unwrap().0.y;
    app.world_mut().get_mut::<Position>(vpt).unwrap().0 = Vec3::new(135.0, y, COURSE_Z);
    app.world_mut()
        .get_mut::<Transform>(vpt)
        .unwrap()
        .translation = Vec3::new(135.0, y, COURSE_Z);
    app.world_mut()
        .entity_mut(vpt)
        .insert(mm2_vehicle::Teleported);

    run(&mut app, 1000);
    let pos = app.world().get::<Position>(vpt).unwrap().0;
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "the missed junction must be rejoined, not abandoned: {:?} cleared {}/1 at {pos:?}",
        progress.state,
        progress.cleared_count()
    );
    assert_eq!(
        app.world().get::<OpponentDriver>(vpt).unwrap().reanchors,
        0,
        "the route chase itself rejoins — no re-anchor was needed"
    );
}

/// Edge: competing recovery positions. Two cars penned in adjacent
/// pockets fill their stuck budgets on the same frame and both project
/// onto the same route spot — the `claimed`/`occupied` walk keeps the
/// second landing [`REANCHOR_CLEAR`] off the first instead of
/// teleporting them into each other.
#[test]
fn competing_reanchors_land_clear_of_each_other() {
    // Both routes run the same z=60 line — far from the z=140 gates so
    // only occupancy, never a pending trigger, can space the landings —
    // with different row-0 stagings so the grid spawn does not overlap.
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n\
         vpheavy race0-a-1.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[
            (
                "race0-a-0.opp",
                opp_file(&[[70.0, 0.0, 60.0], [140.0, 0.0, 60.0], [180.0, 0.0, 60.0]]),
            ),
            (
                "race0-a-1.opp",
                opp_file(&[[76.0, 0.0, 60.0], [140.0, 0.0, 60.0], [184.0, 0.0, 60.0]]),
            ),
        ],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let vpheavy = opponent_by_vehicle(&mut app, "vpheavy");

    // Two 4×4 m pockets sharing the x walls — each under the 8 m
    // displacement bubble so both windows fill together. Same x, so
    // both cars project to the same leg point: without the occupancy
    // check they would claim the same landing.
    let y = app.world().get::<Position>(vpt).unwrap().0.y;
    for cx in [131.8f32, 136.2] {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(0.4, 3.0, 12.0),
            Transform::from_translation(Vec3::new(cx, y + 1.0, 120.0)),
        ));
    }
    for cz in [113.9f32, 118.1, 119.9, 124.1] {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(8.0, 3.0, 0.4),
            Transform::from_translation(Vec3::new(134.0, y + 1.0, cz)),
        ));
    }
    for (e, z) in [(vpt, 116.0f32), (vpheavy, 122.0)] {
        app.world_mut().get_mut::<Position>(e).unwrap().0 = Vec3::new(134.0, y, z);
        app.world_mut().get_mut::<Transform>(e).unwrap().translation = Vec3::new(134.0, y, z);
        app.world_mut()
            .entity_mut(e)
            .insert(mm2_vehicle::Teleported);
        // Each car's guide has it 50 m down the route — abreast of the
        // pens — as if it had been punted off the line there, so both
        // recoveries head for the same stretch.
        app.world_mut()
            .get_mut::<OpponentDriver>(e)
            .unwrap()
            .neural
            .as_mut()
            .expect("a routed entry has a driver")
            .arc = 50.0;
    }

    let mut anchored = [false; 2];
    for _ in 0..1400 {
        app.update();
        anchored = [
            app.world().get::<OpponentDriver>(vpt).unwrap().reanchors > 0,
            app.world()
                .get::<OpponentDriver>(vpheavy)
                .unwrap()
                .reanchors
                > 0,
        ];
        if anchored[0] && anchored[1] {
            break;
        }
    }
    assert!(
        anchored[0] && anchored[1],
        "both penned cars must re-anchor: {anchored:?}"
    );

    // Give the dispatched teleports their pipeline frames.
    run(&mut app, 3);
    let a = app.world().get::<Position>(vpt).unwrap().0;
    let b = app.world().get::<Position>(vpheavy).unwrap().0;
    assert!(
        (a.z - 60.0).abs() < 3.0 && (b.z - 60.0).abs() < 3.0,
        "both land back on the route line: {a:?} {b:?}"
    );
    assert!(
        a.distance(b) >= REANCHOR_CLEAR - 0.5,
        "competing recoveries must not land interpenetrating: {a:?} vs {b:?}"
    );
    for e in [vpt, vpheavy] {
        let rot = app.world().get::<Rotation>(e).unwrap().0;
        assert!((rot * Vec3::Y).y > 0.99, "the landing is upright: {rot:?}");
        assert_eq!(
            app.world().get::<RaceProgress>(e).unwrap().cleared_count(),
            0,
            "the disclosed teleports banked nothing"
        );
    }
}

/// DSN-45 route-bound Ordered progress (UNK-11 — original AI progress
/// semantics unverified): an authored `.opp` line legally misses a
/// gate cylinder — this course's middle gate sits 30 m off the
/// authored lane — and a trigger-only binding stalls the field at
/// that gate forever (the authored-miss defect class the F14-A.2
/// matrix measured on london-2/4/5 and sf-7). Binding the Ordered
/// gates to the driven route's arc covers the miss while physical
/// crossings still bank the gates the car does sweep; the
/// route-derived clears are counted separately so evidence can tell
/// the two apart.
#[test]
fn ordered_route_credit_covers_a_gate_the_authored_line_misses() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,1,0,0.1,0.0,1,50,1,0,0,0,1,0,0.2,0.0,1,40,1\n"),
    );
    write(
        d,
        "race/testcity/circuit0.aimap",
        aimap_with_opponents("vpt circuit0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n"),
    );
    // Ordered needs ≥4 rows: the start line plus three course gates —
    // the definition re-arms a lifted copy of the line as the lap's
    // last gate.
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(60.0, COURSE_Z),
            waypoint_row(95.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(165.0, COURSE_Z),
        ),
    );
    // The first course gate sits on the straight before the swerve —
    // a gate plane on the corner anchor itself is crossed a frame
    // after a driver cutting the corner already projects past it, and
    // route credit would bank it first.
    // The authored line swerves 30 m around the middle gate — its
    // closest approach misses the 15 m cylinder by ~19 m — then
    // rejoins: the authored-miss shape the matrix measured.
    write(
        d,
        "race/testcity/circuit0-a-0.opp",
        opp_file(&[
            [70.0, 0.0, 140.0],
            [110.0, 0.0, 140.0],
            [140.0, 0.0, 170.0],
            [165.0, 0.0, 140.0],
            [180.0, 0.0, 140.0],
        ]),
    );
    write_car(d, "vpt", 1000.0, None);
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Circuit,
            index: 0,
        }),
        ..SessionConfig::default()
    };
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    assert!(
        app.world().get::<RouteGateLine>(vpt).is_some(),
        "an Ordered opponent with a route binds the gate line"
    );
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    assert!(
        app.world().get::<RouteGateLine>(car).is_none(),
        "the local player stays trigger-bound — no route line"
    );

    // ~25 s: the ~137 m route is drivable well inside that at the
    // pace the existing integration legs measure.
    run(&mut app, 1600);
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "route credit must carry the car past the missed gate — state={:?} cleared={} crossings={} route_clears={}",
        progress.state,
        progress.cleared_count(),
        progress.crossings,
        progress.route_clears,
    );
    // Every banked gate is exactly one source: the four Ordered gates
    // (three course plus the re-armed line) split across physical
    // crossings and route-derived clears with no double count.
    assert_eq!(
        progress.crossings + progress.route_clears,
        4,
        "crossings={} + route_clears={} must equal the lap's four gates",
        progress.crossings,
        progress.route_clears,
    );
    assert!(
        progress.route_clears >= 1,
        "the missed-gate/lapped-line clears come from the route binding"
    );
    assert!(
        progress.crossings >= 1,
        "the dead-centre first gate still banks as a real crossing"
    );
}

/// The binding is per-participant and only ever exists where a route
/// does: an Ordered roster entry whose `.opp` never shipped resolves
/// `route: None` (an `UnresolvedRoute` issue, not a build failure) and
/// binds no gate line — it stays trigger-bound and earns nothing.
#[test]
fn ordered_route_less_opponent_binds_no_gate_line() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,1,0,0.1,0.0,1,50,1,0,0,0,1,0,0.2,0.0,1,40,1\n"),
    );
    // The roster references an `.opp` the install does not ship.
    write(
        d,
        "race/testcity/circuit0.aimap",
        aimap_with_opponents("vpt circuit0-a-9.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n"),
    );
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(60.0, COURSE_Z),
            waypoint_row(110.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(165.0, COURSE_Z),
        ),
    );
    write_car(d, "vpt", 1000.0, None);
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Circuit,
            index: 0,
        }),
        ..SessionConfig::default()
    };
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    assert!(
        app.world().get::<RouteGateLine>(vpt).is_none(),
        "no route → no route-bound progress model"
    );
    run(&mut app, 400);
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert_eq!(
        progress.cleared_count(),
        0,
        "route credit cannot fabricate gates"
    );
    assert_eq!(progress.route_clears, 0);
}

#[test]
fn native_dense_opponent_physically_drives_two_tight_laps_without_recovery() {
    let tmp = circuit_install();
    let d = tmp.path();
    // The shared straight-lane fixture steers both axles identically. This
    // corner fixture authors ordinary front-wheel steering, leaving its
    // engine, transmission, brakes, grip and native AI demands unchanged.
    let tune = support::vehcarsim(1000.0);
    let (front, back) = tune.split_once("WheelBack {").unwrap();
    write(
        d,
        "tune/vehicle/vpt.vehcarsim",
        format!(
            "{front}WheelBack {{{}",
            back.replacen("SteeringLimit 0.5", "SteeringLimit 0.0", 1)
        ),
    );

    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,1,0,0,0,2,50,1,0,0,0,1,0,0,0,2,40,1\n"),
    );
    write(
        d,
        "race/testcity/circuit0.aimap",
        aimap_with_opponents("vpt circuit0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n"),
    );
    write(
        d,
        "race/testcity/cir0_strtpnts",
        "-200,0,140,-90,0,0,0,0,0,\n0,0,140,-90,0,0,0,0,0,\n",
    );
    let corners = [
        [0., 0., 140.],
        [35., 0., 140.],
        [35., 0., 105.],
        [0., 0., 105.],
    ];
    let mut points = Vec::new();
    let mut gates = WAYPOINTS.to_string();
    for (i, a) in corners.iter().enumerate() {
        gates.push_str(&waypoint_row(a[0], a[2]));
        let b = corners[(i + 1) % corners.len()];
        for k in 0..7 {
            let t = k as f32 / 7.;
            points.push([a[0] + (b[0] - a[0]) * t, 0., a[2] + (b[2] - a[2]) * t]);
        }
    }
    points.push(corners[0]);
    write(d, "race/testcity/circuit0waypoints.csv", gates);
    write(d, "race/testcity/circuit0-a-0.opp", opp_file(&points));
    let mut app = event_app(circuit_config(), vfs_of(d));
    app.update();
    let ai = opponent_by_vehicle(&mut app, "vpt");
    assert_eq!(
        app.world().get::<Player>(ai).unwrap().control,
        PlayerControl::Ai
    );
    let mut worst_lateral = 0.0_f32;
    for _ in 0..4800 {
        app.update();
        let pos = app.world().get::<Position>(ai).unwrap().0;
        let lateral = corners
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let a = Vec3::from_array(*a);
                let b = Vec3::from_array(corners[(i + 1) % 4]);
                let ab = b - a;
                let t = ((pos - a).dot(ab) / ab.length_squared()).clamp(0., 1.);
                let delta = pos - (a + ab * t);
                delta.x.hypot(delta.z)
            })
            .fold(f32::INFINITY, f32::min);
        worst_lateral = worst_lateral.max(lateral);
        if matches!(
            app.world().get::<RaceProgress>(ai).unwrap().state,
            ParticipantState::Finished { .. }
        ) {
            break;
        }
    }
    println!("physical native dense AI worst_lateral={worst_lateral}");
    let progress = app.world().get::<RaceProgress>(ai).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "{:?} lap={} lateral={worst_lateral}",
        progress.state,
        progress.lap
    );
    assert_eq!(progress.lap, 2);
    let driver = app.world().get::<OpponentDriver>(ai).unwrap();
    assert_eq!(driver.reanchors, 0);
    assert_eq!(
        driver.neural.as_ref().map_or(0, |n| n.unstick.count),
        0,
        "no back-ups"
    );
    assert!(
        worst_lateral < 15.0,
        "the car left the course at a corner: {worst_lateral}"
    );
}

/// Recovery resumes the authored occurrence on revisited streets, rather
/// than choosing the first globally forward anchor from a fresh spawn.
#[test]
fn native_reanchor_preserves_revisited_street_occurrence() {
    let tmp = roster_install(
        "",
        &[(
            "race0-a-0.opp",
            opp_file(&[
                [-100.0, 0.0, 140.0],
                [-50.0, 0.0, 140.0],
                [0.0, 0.0, 140.0],
                [50.0, 0.0, 140.0],
                [50.0, 0.0, 50.0],
                [0.0, 0.0, 50.0],
                [0.0, 0.0, 140.0],
                [50.0, 0.0, 140.0],
                [100.0, 0.0, 140.0],
                [180.0, 0.0, 140.0],
            ]),
        )],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    run(&mut app, 260);
    let before = Vec3::new(25.0, 0.8, 140.0);
    app.world_mut().get_mut::<Position>(vpt).unwrap().0 = before;
    app.world_mut().get_mut::<Rotation>(vpt).unwrap().0 =
        Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    app.world_mut().get_mut::<LinearVelocity>(vpt).unwrap().0 = Vec3::ZERO;
    {
        // The car's guide has it on the *second* pass of the street
        // (source points 6 → 7), 25 m in.
        let mut d = app.world_mut().get_mut::<OpponentDriver>(vpt).unwrap();
        let neural = d.neural.as_mut().expect("a routed entry has a driver");
        neural.arc = neural.guide.source_arc(6) + 25.0;
        d.stuck_pos = before;
        d.stuck_frames = REANCHOR_FRAMES - 1;
    }
    let cleared = app
        .world()
        .get::<RaceProgress>(vpt)
        .unwrap()
        .cleared_count();
    run(&mut app, 4);
    let d = app.world().get::<OpponentDriver>(vpt).unwrap();
    assert_eq!(d.reanchors, 1);
    assert_eq!(d.next, 7, "the landing belongs to the later occurrence");
    assert_eq!(
        app.world()
            .get::<RaceProgress>(vpt)
            .unwrap()
            .cleared_count(),
        cleared,
        "recovery must not bank checkpoint credits"
    );
    // And it resumes down the later pass: on through the three gates,
    // which lie only on that last stretch of the route. (It was put
    // there by the test, not driven there, so its route-bound progress
    // is 300 m short and the finish stays a physical crossing.)
    run(&mut app, 1800);
    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert_eq!(
        (progress.cleared_count(), progress.crossings),
        (3, 3),
        "the car drives the rest of the later pass: {progress:?}"
    );
    assert!(app.world().get::<Position>(vpt).unwrap().0.x > 150.0);
}

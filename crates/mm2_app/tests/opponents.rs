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
    Blocker, DriveStats, OpponentDriver, REANCHOR_CLEAR, REANCHOR_FRAMES, Traffic, apply_gap_brake,
    apply_rear_end_guard, initial_route_index, nearest_blocker, opponent_drive, pick_pass_side,
    reanchor_pose, reanchor_pose_with_progress, rear_end_demand, route_target, spawn_pose,
    trim_behind_staging,
};
use mm2_app::racing_line::CarLimits;
use mm2_app::scripted::{ScriptedBot, ScriptedTuning};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts, race};
use mm2_assets::Vfs;
use mm2_game::{
    Densities, Difficulty, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ObjectIdentity,
    OpponentRoute, OpponentRoutePoint, OpponentSpec, ParticipantState, Player, PlayerControl,
    PlayerVehicle, RaceCustomization, RaceDefinition, RaceProgress, RaceStarted, RaceState,
    ResultLedger, RouteGateLine, Session, SessionAuthority, SessionConditions, SessionConfig,
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

#[test]
fn route_target_advances_past_reached_points() {
    let r = route(&[[0.0, 0.0, 0.0], [50.0, 0.0, 0.0], [100.0, 0.0, 0.0]]);
    // Standing on the first point chases the next.
    let (next, target) = route_target(&r, 0, Vec3::new(0.0, 0.0, 0.0));
    assert_eq!(next, 1);
    assert_eq!(target, Some(Vec3::new(50.0, 0.0, 0.0)));
    // Within reach of point 1 → point 2.
    let (next, target) = route_target(&r, 1, Vec3::new(40.0, 0.0, 3.0));
    assert_eq!(next, 2);
    assert_eq!(target, Some(Vec3::new(100.0, 0.0, 0.0)));
}

#[test]
fn route_target_skips_a_point_the_car_drove_past() {
    let r = route(&[[0.0, 0.0, 0.0], [50.0, 0.0, 0.0], [100.0, 0.0, 0.0]]);
    // 8 m beyond point 1 laterally offset — past the perpendicular —
    // the chase must move on to point 2, not U-turn back.
    let (next, target) = route_target(&r, 1, Vec3::new(58.0, 0.0, 30.0));
    assert_eq!(next, 2);
    assert_eq!(target, Some(Vec3::new(100.0, 0.0, 0.0)));
}

#[test]
fn route_target_open_route_completes() {
    let r = route(&[[0.0, 0.0, 0.0], [50.0, 0.0, 0.0], [100.0, 0.0, 0.0]]);
    let (next, target) = route_target(&r, 2, Vec3::new(95.0, 0.0, 0.0));
    assert_eq!(next, 3);
    assert_eq!(target, None, "open route past its end drives nothing");
}

/// A route whose row-0 `brake` carries the authored staging heading —
/// the measured meaning of a nonzero value on retail `.opp` files.
fn staged_route(heading_deg: f32, points: &[[f32; 3]]) -> OpponentRoute {
    let mut r = route(points);
    r.points[0].brake = heading_deg;
    r
}

#[test]
fn route_target_closed_route_loops_to_nearest() {
    // A circuit: the last anchor sits 10 m from the first.
    let r = route(&[
        [0.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [100.0, 0.0, 100.0],
        [0.0, 0.0, 100.0],
        [0.0, 0.0, 10.0],
    ]);
    // Just past the last anchor the chase wraps: point 0 is also in
    // reach, so the target becomes point 1 — the next lap's first leg.
    let (next, target) = route_target(&r, 4, Vec3::new(0.0, 0.0, 12.0));
    assert_eq!(next, 1, "closed route rejoins on the next lap");
    assert_eq!(target, Some(Vec3::new(100.0, 0.0, 0.0)));
}

#[test]
fn spawn_pose_prefers_the_authored_grid_then_the_route_anchor() {
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
    let (pos, yaw) = spawn_pose(&def, 0, &spec, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(56.0, 0.0, 144.0), "authored slot 1 wins");
    // The slot's own authored `a` is the facing — vehicle-yaw degrees —
    // not the route leg (+X would give −π/2).
    assert!(
        (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "slot spawn faces its authored yaw, got {yaw}"
    );

    // No authored grid → the route's row-0 anchors the spawn; with no
    // authored staging heading the first leg still supplies the facing.
    def.start_slots.truncate(1);
    let (pos, yaw) = spawn_pose(&def, 0, &spec, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 140.0), "route row0 anchors");
    assert!(
        (yaw + std::f32::consts::FRAC_PI_2).abs() < 0.35,
        "no authored heading → the +X first leg faces it, got {yaw}"
    );

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

/// A grid slot carrying no authored heading (`yaw_deg == None` — the
/// producer's `_strtpnts` `a = 0` mapping, retail `cir6`'s all-zero
/// column) still spawns on the slot but takes the route's staged
/// heading — a verbatim −Z would face it backward off the measured
/// ~180° course. With no staged heading the first leg supplies the
/// facing, like a route-anchor spawn.
#[test]
fn spawn_pose_on_a_headless_grid_slot_uses_the_route_facing() {
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
    assert_eq!(pos, Vec3::new(56.0, 0.0, 144.0), "the authored slot stands");
    assert!(
        fwd(yaw).z > 0.98,
        "no authored slot yaw → the route's 180° staging faces +Z, got {yaw}"
    );

    // No staged heading either → the route's first leg supplies it:
    // the nearest anchor sits +X of the slot.
    let unstaged = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: Some(route(&[[70.0, 0.0, 144.0], [70.0, 0.0, 170.0]])),
    };
    let (pos, yaw) = spawn_pose(&def, 0, &unstaged, Vec3::new(60.0, 0.0, 140.0), 0.0);
    assert_eq!(pos, Vec3::new(56.0, 0.0, 144.0));
    assert!(
        fwd(yaw).x > 0.98,
        "no authored headings at all → the first-leg facing, got {yaw}"
    );

    // A route-less entry on a headless slot inherits the player's yaw.
    let bare = OpponentSpec {
        vehicle: "vpt".into(),
        params: Vec::new(),
        route: None,
    };
    let (_, yaw) = spawn_pose(&def, 0, &bare, Vec3::new(60.0, 0.0, 140.0), 0.4);
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
/// authored facing wins, every +X anchor is left behind the line and
/// the car holds its staged nose instead of reversing into the tail.
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

    // Past the countdown there is still nothing ahead to chase — the
    // car holds its line rather than U-turning back for row 1.
    run(&mut app, 400);
    let input = app.world().get::<VehicleInput>(vpt).unwrap();
    assert_eq!(
        (input.throttle, input.steering),
        (0.0, 0.0),
        "no ahead target → no drive demand"
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
        let slot = app.world().resource::<RaceState>().definition.start_slots[d.index + 1].position;
        assert!(
            (pos.x - slot.x).abs() < 3.0 && (pos.z - slot.z).abs() < 3.0,
            "opponent {} back on its authored grid slot, got {pos:?}",
            d.index
        );
        let route = d.route.as_ref().expect("circuit route resolved");
        let line = app
            .world()
            .get::<RouteGateLine>(e)
            .expect("an Ordered roster entry carries the route bind");
        let fresh = RouteGateLine::bind(
            route,
            &app.world().resource::<RaceState>().definition.checkpoints,
            line.is_closed(),
            pos,
            d.next,
        )
        .expect("the same bind the spawn ran");
        assert_eq!(
            line.arc_high, fresh.arc_high,
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

fn drain_impacts(app: &mut App) -> Vec<ImpactEvent> {
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .drain()
        .collect()
}

// ---------------------------------------------------------------------------
// F15-B.1 — traffic avoidance / overtake
// ---------------------------------------------------------------------------

fn traffic(e: u32, pos: [f32; 3], fwd: [f32; 3], speed: f32) -> Traffic {
    Traffic {
        entity: Entity::from_raw_u32(e).unwrap(),
        control: PlayerControl::Local,
        pos: Vec3::from_array(pos),
        fwd: Vec3::from_array(fwd),
        speed,
    }
}

#[test]
fn nearest_blocker_reads_only_the_corridor_ahead() {
    let me = Entity::from_raw_u32(0).unwrap();
    let pos = Vec3::ZERO;
    let fwd = Vec3::NEG_Z;
    let reach = 50.0;
    let self_entry = traffic(0, [0.0, 0.0, -5.0], [0.0, 0.0, -1.0], 0.0);
    let ahead = traffic(1, [0.0, 0.0, -20.0], [0.0, 0.0, -1.0], 10.0);
    let near = traffic(2, [1.0, 0.0, -12.0], [0.0, 0.0, -1.0], 0.0);
    let next_lane = traffic(3, [6.0, 0.0, -10.0], [0.0, 0.0, -1.0], 10.0);
    let behind = traffic(4, [0.0, 0.0, 5.0], [0.0, 0.0, -1.0], 10.0);
    let far = traffic(5, [0.0, 0.0, -80.0], [0.0, 0.0, -1.0], 10.0);
    let all = [self_entry, ahead, near, next_lane, behind, far];

    let b = nearest_blocker(me, pos, fwd, reach, &all, |_| true).unwrap();
    assert_eq!(b.entity, all[2].entity, "the nearer corridor car wins");
    assert!((b.gap - 12.0).abs() < 0.01);
    assert!(b.lat > 0.0, "x=+1 sits to the right of a -Z heading");
    assert_eq!(b.speed, 0.0, "parked blocker reads no closing pace");

    let all = [self_entry, all[1], all[3], all[4], all[5]];
    let b = nearest_blocker(me, pos, fwd, reach, &all, |_| true).unwrap();
    assert_eq!(b.entity, all[1].entity, "next-nearest corridor car");
    assert!((b.speed - 10.0).abs() < 0.01);

    // Off the corridor entirely: nothing ahead.
    let clear = [traffic(6, [4.0, 0.0, -10.0], [0.0, 0.0, -1.0], 0.0)];
    assert!(
        nearest_blocker(me, pos, fwd, reach, &clear, |_| true).is_none(),
        "a car on the next lane is not a blocker"
    );
}

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

/// The rear-end guard's demand is the share of the car's braking that
/// sheds the closure before the stopping gap: nothing when not
/// closing, nothing for a standing car approached at a crawl (the
/// escape handles that), and more the faster and nearer the closure.
#[test]
fn rear_end_guard_brakes_for_the_closure_it_must_shed() {
    let limits = CarLimits::of(&VehicleConfig::default(), None);
    let ahead = |gap: f32, speed: f32| Blocker {
        entity: Entity::PLACEHOLDER,
        gap,
        lat: 0.0,
        speed,
    };
    assert_eq!(rear_end_demand(&ahead(20.0, 15.0), 12.0, &limits), 0.0);
    assert_eq!(rear_end_demand(&ahead(8.0, 0.0), 1.0, &limits), 0.0);
    let far = rear_end_demand(&ahead(40.0, 0.0), 15.0, &limits);
    let near = rear_end_demand(&ahead(15.0, 0.0), 15.0, &limits);
    assert!(far > 0.0 && near > far, "far={far} near={near}");

    // A hard closure brakes at its demand; a gentle one only lifts.
    let mut input = VehicleInput {
        throttle: 1.0,
        ..Default::default()
    };
    apply_rear_end_guard(&mut input, &ahead(15.0, 0.0), 15.0, &limits);
    assert_eq!(input.throttle, 0.0);
    assert!(input.brake >= near.min(1.0) - 1e-6, "{input:?}");
    let mut input = VehicleInput {
        throttle: 1.0,
        ..Default::default()
    };
    apply_rear_end_guard(&mut input, &ahead(60.0, 10.0), 13.0, &limits);
    assert_eq!((input.throttle, input.brake), (1.0, 0.0), "{input:?}");
}

/// A bare driver for the sense-gate tests: authored spec, default
/// tuning, no committed pass — only the avoid flags vary.
fn driver(avoid_players: bool, avoid_opponents: bool) -> OpponentDriver {
    OpponentDriver {
        index: 0,
        spec: OpponentSpec {
            vehicle: "vpt".into(),
            params: vec![],
            route: None,
        },
        route: None,
        tuning: ScriptedTuning::DEFAULT,
        avoid_players,
        avoid_opponents,
        look_ahead: 75.0,
        next: 0,
        recovery: ScriptedBot::default(),
        pass_entity: None,
        pass_side: 0.0,
        clear_frames: 0,
        stall_frames: 0,
        stall_pos: Vec3::ZERO,
        pass_ban: None,
        stuck_pos: Vec3::ZERO,
        stuck_frames: 0,
        stuck_peak: 0,
        reanchors: 0,
        catch_up_policy: mm2_game::CatchUpPolicy::default(),
        catch_up: 0.0,
        limits: CarLimits::of(&VehicleConfig::default(), None),
        corner_brake: mm2_app::racing_line::CORNER_BRAKE_DEFAULT,
        stats: Default::default(),
    }
}

/// The authored avoid flags gate which participant classes the
/// corridor sees (F15-B.2/B.8): an opponent that does not avoid
/// players drives through a parked human car as if it were not there,
/// and an authored-0 `avoidOpponents` makes fellow AI transparent the
/// same way — 59% of retail rows author exactly that (documented
/// polarity, R3/R4).
#[test]
fn authored_avoid_flags_gate_the_corridor() {
    let me = Entity::from_raw_u32(0).unwrap();
    let pos = Vec3::ZERO;
    let fwd = Vec3::NEG_Z;
    let reach = 50.0;
    let human = traffic(1, [0.0, 0.0, -20.0], [0.0, 0.0, -1.0], 0.0);
    let mut ai = traffic(2, [0.0, 0.0, -30.0], [0.0, 0.0, -1.0], 0.0);
    ai.control = PlayerControl::Ai;
    let all = [human, ai];

    // Sensing both classes: the nearer human car blocks.
    let aware = driver(true, true);
    let b = nearest_blocker(me, pos, fwd, reach, &all, |t| aware.senses(t));
    assert_eq!(b.unwrap().entity, human.entity);

    // avoidPlayers off: the human is transparent, the AI blocks.
    let deaf = driver(false, true);
    let b = nearest_blocker(me, pos, fwd, reach, &all, |t| deaf.senses(t));
    assert_eq!(b.unwrap().entity, ai.entity);
    assert!(
        nearest_blocker(me, pos, fwd, reach, &all[..1], |t| deaf.senses(t)).is_none(),
        "a driver that does not avoid players sees nobody in a human-only lane"
    );

    // avoidOpponents off: the AI is transparent, the human blocks.
    let feral = driver(true, false);
    let b = nearest_blocker(me, pos, fwd, reach, &all, |t| feral.senses(t));
    assert_eq!(b.unwrap().entity, human.entity);
    assert!(
        nearest_blocker(me, pos, fwd, reach, &all[1..], |t| feral.senses(t)).is_none(),
        "an authored-0 avoidOpponents blinds the driver to AI"
    );

    // Both off: nothing is sensed at all.
    let blind = driver(false, false);
    assert!(nearest_blocker(me, pos, fwd, reach, &all, |t| blind.senses(t)).is_none());
}

#[test]
fn pick_pass_side_prefers_the_open_side() {
    assert_eq!(
        pick_pass_side(2.0, 0.0),
        -1.0,
        "blocker on the right → pass left"
    );
    assert_eq!(
        pick_pass_side(-2.0, 0.0),
        1.0,
        "blocker on the left → pass right"
    );
    assert_eq!(
        pick_pass_side(0.0, -2.0),
        -1.0,
        "dead-centre → the side the route continues"
    );
    assert_eq!(
        pick_pass_side(0.0, 0.0),
        1.0,
        "dead-centre on a straight route → the default"
    );
}

#[test]
fn gap_brake_bites_only_inside_the_comfort_gap() {
    let parked_far = Blocker {
        entity: Entity::PLACEHOLDER,
        gap: 40.0,
        lat: 0.0,
        speed: 0.0,
    };
    let mut input = VehicleInput {
        throttle: 1.0,
        ..default()
    };
    apply_gap_brake(&mut input, &parked_far, 20.0);
    assert_eq!(input.throttle, 1.0, "outside the gap the demand stands");
    assert_eq!(input.brake, 0.0);

    // Inside the gap and closing hard: throttle cut, brake applied.
    let parked_near = Blocker {
        gap: 6.0,
        ..parked_far
    };
    let mut input = VehicleInput {
        throttle: 1.0,
        ..default()
    };
    apply_gap_brake(&mut input, &parked_near, 20.0);
    assert_eq!(input.throttle, 0.0, "closing inside the gap lifts off");
    assert!(
        input.brake > 0.5,
        "a hard close brakes hard: {}",
        input.brake
    );

    // Matched speed in the outer half of the gap: hold station rather
    // than flap brake/coast.
    let matched = Blocker {
        gap: 10.0,
        speed: 18.0,
        ..parked_far
    };
    let mut input = VehicleInput {
        throttle: 0.4,
        ..default()
    };
    apply_gap_brake(&mut input, &matched, 18.0);
    assert_eq!(
        input.throttle, 0.4,
        "station-keeping leaves the demand alone"
    );
    assert_eq!(input.brake, 0.0);

    // Matched speed *inside* the comfort gap: a moving blocker gets a
    // soft adaptive-cruise brake so the queue keeps a gap instead of
    // riding bumpers.
    let tailgated = Blocker {
        gap: 5.0,
        ..matched
    };
    let mut input = VehicleInput {
        throttle: 0.4,
        ..default()
    };
    apply_gap_brake(&mut input, &tailgated, 18.0);
    assert_eq!(input.throttle, 0.0, "a moving blocker in the gap lifts off");
    assert!(
        input.brake > 0.0 && input.brake < 0.5,
        "gap-keeping is a soft brake, not a panic stop: {}",
        input.brake
    );

    // A standing blocker at crawl pace does not brake — braking there
    // is what deadlocked the follower behind parked cars; the pass
    // steering is the avoidance.
    let crawl = Blocker {
        gap: 6.0,
        ..parked_far
    };
    let mut input = VehicleInput {
        throttle: 0.2,
        ..default()
    };
    apply_gap_brake(&mut input, &crawl, 1.0);
    assert_eq!(input.throttle, 0.2, "crawl pace keeps steering priority");
    assert_eq!(input.brake, 0.0);
}

/// A participant parked on the route is an obstacle, not a wall
/// (F15-B.1, AC03's blocked-road leg): `vpheavy`'s `.opp` is a single
/// point parked mid-lane on `vpt`'s line, so it holds still while the
/// follower commits a pass side, drives around without contact and
/// still finishes the course.
#[test]
fn blocked_route_drives_around_the_parked_car() {
    let tmp = roster_install(
        "",
        &[("race0-a-1.opp", opp_file(&[[100.0, 0.0, COURSE_Z]]))],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    let heavy_obj = app.world().get::<ObjectIdentity>(heavy).unwrap().0;
    let heavy_start = app.world().get::<Position>(heavy).unwrap().0;

    let mut impacts = Vec::new();
    let mut max_dev = 0.0f32;
    for _ in 0..1800 {
        app.update();
        impacts.extend(drain_impacts(&mut app));
        let p = app.world().get::<Position>(vpt).unwrap().0;
        max_dev = max_dev.max((p.z - COURSE_Z).abs());
    }

    let progress = app.world().get::<RaceProgress>(vpt).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "the follower still finishes around the obstacle: {:?} cleared {}/3",
        progress.state,
        progress.cleared_count()
    );
    assert!(
        max_dev > 1.0,
        "the pass visibly left the lane line: max |z-140| = {max_dev}"
    );
    assert!(
        impacts
            .iter()
            .all(|e| e.participants.0 != heavy_obj && e.participants.1 != heavy_obj),
        "avoidance drives around — no contact with the parked car"
    );
    let heavy_pos = app.world().get::<Position>(heavy).unwrap().0;
    assert!(
        (heavy_pos - heavy_start).length() < 3.0,
        "the obstacle was not shoved down the lane: {heavy_start:?} → {heavy_pos:?}"
    );
    assert_eq!(
        app.world().get::<RaceProgress>(heavy).unwrap().state,
        ParticipantState::Racing,
        "the parked entry stays a participant, unresolved"
    );
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
/// path: `maxThrottle` clamps to the input ceiling, the corner-speed
/// column scales the speed plan's corner grip by `value / 2.0` and the
/// corner-brake column is its brake-demand floor, the look-ahead column sets
/// the corridor reach, and `avoidPlayers`/`avoidOpponents` gate the
/// corridor's classes — `None` columns take the `RegisterRoute`
/// defaults.
#[test]
fn authored_tail_binds_the_driver_tuning() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 0.90 0 50.0 0.4 1 1 0 1 0 1.5\nvpheavy race0-a-1.opp 0.80 0 120.0 0.7 1 1 1 0 0 2.0\n",
        &[],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let d = app.world().get::<OpponentDriver>(vpt).unwrap();
    assert!((d.tuning.throttle_cap - 0.9).abs() < 1e-6);
    let cfg = &app.world().get::<Vehicle>(vpt).unwrap().config;
    let stock = CarLimits::of(cfg, None);
    assert!(
        (d.limits.corner_accel / stock.corner_accel - 0.75).abs() < 1e-4,
        "col 9 scales the planned corner grip: {:?} vs {stock:?}",
        d.limits
    );
    assert_eq!(d.corner_brake, 0.4, "col 3 is the brake-demand floor");
    assert!(
        !d.avoid_players,
        "authored avoidPlayers=0 (col 6) gates human sensing off"
    );
    assert!(d.avoid_opponents, "authored avoidOpponents=1 (col 7)");
    assert_eq!(d.look_ahead, 50.0, "col 2 binds the corridor reach");

    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    let d = app.world().get::<OpponentDriver>(heavy).unwrap();
    assert!((d.tuning.throttle_cap - 0.8).abs() < 1e-6);
    let cfg = &app.world().get::<Vehicle>(heavy).unwrap().config;
    assert_eq!(d.limits, CarLimits::of(cfg, None), "2.0 is the default");
    assert_eq!(d.corner_brake, 0.7);
    assert!(d.avoid_players);
    assert!(
        !d.avoid_opponents,
        "authored avoidOpponents=0 (col 7) gates AI sensing off"
    );
    assert_eq!(d.look_ahead, 120.0);
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
        .map(|(d, p)| (d.tuning.throttle_cap, p.0.x))
        .collect();
    by_cap.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (slow_x, fast_x) = (by_cap[0].1, by_cap[1].1);
    assert!(
        fast_x - slow_x > 10.0,
        "authored maxThrottle must measure: 0.5-cap x={slow_x:.1} vs 1.0-cap x={fast_x:.1}"
    );
}

/// `avoidPlayers` (tail column 6) gates the corridor through the full
/// driving system: with the local car parked mid-lane, an authored-1
/// driver commits a pass and drives around without contact; the
/// authored-0 twin never senses it and collides head-on.
#[test]
fn authored_avoid_players_decides_the_parked_player() {
    for (tail, expect_contact) in [("1 1 1 1 0", false), ("1 1 0 1 0", true)] {
        let tmp = roster_install_rows(
            &format!("vpt race0-a-0.opp 0.90 0 50.0 0.7 {tail} 1.0\n"),
            &[],
        );
        let mut app = event_app(event_config(), vfs_of(tmp.path()));
        app.update();
        let vpt = opponent_by_vehicle(&mut app, "vpt");
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerVehicle>>()
            .iter(app.world())
            .next()
            .unwrap();
        let player_obj = app.world().get::<ObjectIdentity>(player).unwrap().0;
        // The grid hold re-asserts the slot pose until the green —
        // park the local car dead-centre on vpt's lane once it is over.
        run(&mut app, 240);
        app.world_mut().get_mut::<Position>(player).unwrap().0 = Vec3::new(110.0, 0.0, COURSE_Z);

        let mut impacts = Vec::new();
        let mut max_dev = 0.0f32;
        for _ in 0..460 {
            app.update();
            impacts.extend(drain_impacts(&mut app));
            let p = app.world().get::<Position>(vpt).unwrap().0;
            max_dev = max_dev.max((p.z - COURSE_Z).abs());
        }
        let hit = impacts
            .iter()
            .any(|e| e.participants.0 == player_obj || e.participants.1 == player_obj);
        assert_eq!(
            hit, expect_contact,
            "avoidPlayers tail {tail}: contact with the parked player = {hit}"
        );
        if expect_contact {
            continue;
        }
        let p = app.world().get::<Position>(vpt).unwrap().0;
        assert!(
            p.x > 115.0 && max_dev > 0.5,
            "the sensing driver slips past the parked car off the lane line: pos={p:?} max_dev={max_dev}"
        );
    }
}

/// `avoidOpponents` (tail column 7) gates AI sensing through the full
/// driving system: a route-less roster slot parked mid-lane is an
/// authored fellow opponent — the authored-1 driver commits a pass and
/// slips around it; the authored-0 twin never senses it for a pass and
/// holds its lane into it, but the rear-end guard (DSN-66) brakes it
/// down first, so the contact is a nudge (~2 m/s) where it used to be
/// a 17 m/s crash.
#[test]
fn authored_avoid_opponents_decides_the_parked_ai() {
    for (tail, expect_contact) in [("1 1 1 1 0", false), ("1 1 1 0 0", true)] {
        let tmp = roster_install_rows(
            &format!(
                "vpt race0-a-0.opp 0.90 0 50.0 0.7 {tail} 1.0\n\
                 vpheavy race0-a-dead.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n"
            ),
            &[],
        );
        let mut app = event_app(event_config(), vfs_of(tmp.path()));
        app.update();
        let vpt = opponent_by_vehicle(&mut app, "vpt");
        // `vpheavy`'s `.opp` resolves nowhere — the roster keeps the
        // slot but it has no route, so `opponent_drive` holds it still:
        // a parked fellow opponent, not a driven one.
        let parked = opponent_by_vehicle(&mut app, "vpheavy");
        assert!(
            app.world()
                .get::<OpponentDriver>(parked)
                .unwrap()
                .route
                .is_none(),
            "the dead .opp reference leaves the slot route-less"
        );
        let parked_obj = app.world().get::<ObjectIdentity>(parked).unwrap().0;

        run(&mut app, 240);
        app.world_mut().get_mut::<Position>(parked).unwrap().0 = Vec3::new(110.0, 0.0, COURSE_Z);

        let mut impacts = Vec::new();
        let mut max_dev = 0.0f32;
        for _ in 0..460 {
            app.update();
            impacts.extend(drain_impacts(&mut app));
            let p = app.world().get::<Position>(vpt).unwrap().0;
            max_dev = max_dev.max((p.z - COURSE_Z).abs());
        }
        let hit = impacts
            .iter()
            .any(|e| e.participants.0 == parked_obj || e.participants.1 == parked_obj);
        assert_eq!(
            hit, expect_contact,
            "avoidOpponents tail {tail}: contact with the parked AI = {hit}"
        );
        if expect_contact {
            let worst = impacts
                .iter()
                .filter(|e| e.participants.0 == parked_obj || e.participants.1 == parked_obj)
                .map(|e| e.severity)
                .fold(0.0f32, f32::max);
            assert!(
                worst < 5.0,
                "the rear-end guard turns the head-on into a nudge: {worst} m/s"
            );
            continue;
        }
        let p = app.world().get::<Position>(vpt).unwrap().0;
        assert!(
            p.x > 115.0 && max_dev > 0.5,
            "the sensing driver slips past the parked AI off the lane line: pos={p:?} max_dev={max_dev}"
        );
    }
}

/// The authored look-ahead distance (tail column 2) sets how far out
/// the corridor senses: an authored-150 driver commits its pass while
/// the parked blocker is still ~90 m away; the authored-30 twin does
/// not react inside the same window — the blocker has not entered its
/// corridor yet. The blocker is a route-less roster slot (a parked
/// fellow opponent), not the local car — teleporting the player this
/// far ahead sweeps the finish trigger and ends the session.
#[test]
fn authored_look_ahead_sets_the_sensing_distance() {
    use bevy::ecs::system::RunSystemOnce;

    for (lookahead, expect_commit) in [(150.0f32, true), (30.0, false)] {
        let tmp = roster_install_rows(
            &format!(
                "vpt race0-a-0.opp 0.90 0 {lookahead} 0.7 1 1 1 1 0 1.0\n\
                 vpheavy race0-a-dead.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n"
            ),
            &[],
        );
        let mut app = event_app(event_config(), vfs_of(tmp.path()));
        app.update();
        let vpt = opponent_by_vehicle(&mut app, "vpt");
        let parked = opponent_by_vehicle(&mut app, "vpheavy");
        assert!(
            app.world()
                .get::<OpponentDriver>(parked)
                .unwrap()
                .route
                .is_none(),
            "the dead .opp reference leaves the slot route-less"
        );

        // Finish the production countdown, then hold a controlled pose
        // while invoking the real decision system. This imported-tail
        // test isolates sensing from acceleration, steering, race-finish
        // triggers and the finite dev ground exercised by driving tests.
        run(&mut app, mm2_game::DEFAULT_COUNTDOWN_TICKS as usize + 2);
        assert_eq!(
            app.world().get::<RaceProgress>(vpt).unwrap().state,
            ParticipantState::Racing
        );
        let start = Vec3::new(70.0, 1.0, COURSE_Z);
        let along = Vec3::X;
        app.world_mut().get_mut::<Position>(vpt).unwrap().0 = start;
        app.world_mut().get_mut::<Rotation>(vpt).unwrap().0 =
            Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        {
            let mut state = app
                .world_mut()
                .get_mut::<mm2_vehicle::VehicleState>(vpt)
                .unwrap();
            state.forward_speed = 20.0;
            state.grounded = true;
        }
        let fwd = app.world().get::<Rotation>(vpt).unwrap().0 * Vec3::NEG_Z;
        let blocker_pos = start + along * 90.0;
        app.world_mut().get_mut::<Position>(parked).unwrap().0 = blocker_pos;
        app.world_mut()
            .get_mut::<mm2_vehicle::VehicleState>(parked)
            .unwrap()
            .forward_speed = 0.0;
        let traffic = [Traffic {
            entity: parked,
            control: PlayerControl::Ai,
            pos: blocker_pos,
            fwd: along,
            speed: 0.0,
        }];
        assert!(nearest_blocker(vpt, start, fwd, 150.0, &traffic, |_| true).is_some());
        assert!(nearest_blocker(vpt, start, fwd, 30.0, &traffic, |_| true).is_none());

        // Inspect the production sensing decision. Lateral displacement
        // also depends on tyre response and acceleration, so it cannot
        // independently identify whether look-ahead sensed the blocker.
        let decision_dt = app.world().resource::<Time>().delta_secs();
        assert!((decision_dt - 1.0 / 60.0).abs() < 1e-6);
        // RunSystemOnce keeps the same render delta (1/60 s) and the
        // system increments its per-frame decision state each call;
        // neither physics nor the race clock advances the held poses.
        for frame in 0..90 {
            app.world_mut().run_system_once(opponent_drive).unwrap();
            let driver = app.world().get::<OpponentDriver>(vpt).unwrap();
            let passing_blocker = driver.pass_entity == Some(parked) && driver.pass_side != 0.0;
            assert_eq!(
                passing_blocker, expect_commit,
                "look-ahead {lookahead}: production pass commitment on decision frame {frame}"
            );
            let p = app.world().get::<Position>(vpt).unwrap().0;
            let blocker = app.world().get::<Position>(parked).unwrap().0;
            assert!(
                (blocker.x - p.x).hypot(blocker.z - p.z) > 30.0,
                "the negative sighting must stay outside the 30 m range"
            );
        }
    }
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
    // (134,·,160) — beyond every gate's z reach — and move the car in
    // through the production teleport contract (`Teleported` breaks
    // the swept segment, so even this test jump cannot bank a gate).
    let y = app.world().get::<Position>(vpt).unwrap().0.y;
    for (center, size) in [
        (Vec3::new(132.0, y + 1.0, 160.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(136.0, y + 1.0, 160.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(134.0, y + 1.0, 158.0), Vec3::new(8.0, 3.0, 0.4)),
        (Vec3::new(134.0, y + 1.0, 162.0), Vec3::new(8.0, 3.0, 0.4)),
    ] {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Transform::from_translation(center),
        ));
    }
    app.world_mut().get_mut::<Position>(vpt).unwrap().0 = Vec3::new(134.0, y, 160.0);
    app.world_mut()
        .get_mut::<Transform>(vpt)
        .unwrap()
        .translation = Vec3::new(134.0, y, 160.0);
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
            assert!(
                d.recovery.escapes >= 1,
                "the penned car counted its failed blind escapes: {}",
                d.recovery.escapes
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
    assert!(
        (pose.z - COURSE_Z).abs() < 2.0 && pose.x > 70.0 && pose.x < 130.0,
        "re-anchored onto the route behind the pocket: {pose:?}"
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
        d.pass_side = 1.0;
        d.pass_entity = Some(vpt); // stale pass state must clear
    }
    run(&mut app, 4);

    let d = app.world().get::<OpponentDriver>(vpt).unwrap();
    assert_eq!(d.reanchors, 1, "the spent budget fired once");
    assert_eq!(d.pass_side, 0.0);
    assert_eq!(d.pass_entity, None);
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

/// F15-B.4 (designed, DSN-27): a driver trailing the leader lifts its
/// demand ceiling by the disclosed bounded factor — observable on
/// `driver.catch_up` and through `VehicleInput` — while a car at the
/// front gets nothing, the local player is never a recipient, and no
/// checkpoint progress is granted. The parked player supplies the
/// deficit's other end first, then the lead: the assist measures the
/// gap to whoever is actually ahead, AI or human.
#[test]
fn catch_up_lifts_a_trailing_opponents_demand() {
    let tmp = roster_install("", &[]);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let heavy = opponent_by_vehicle(&mut app, "vpheavy");
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    assert!(
        app.world().get::<OpponentDriver>(car).is_none(),
        "the player is never a catch-up recipient"
    );

    // The countdown holds the field — and the assist: nobody races.
    run(&mut app, 60);
    for e in [vpt, heavy] {
        assert_eq!(
            app.world().get::<OpponentDriver>(e).unwrap().catch_up,
            0.0,
            "no assist before the race releases"
        );
    }

    // Racing: the driving opponents lead the parked player — leaders
    // are never lifted (the assist is one-directional by design).
    run(&mut app, 200);
    for e in [vpt, heavy] {
        assert_eq!(
            app.world().get::<OpponentDriver>(e).unwrap().catch_up,
            0.0,
            "the leader earns no assist"
        );
    }

    // Teleport the player deep into the course: the swept segment
    // clears all three gates through the shared `advance` validation
    // but ends 10 m short of the finish trigger — the participant
    // leads at ~3.3 gate units without resolving.
    app.world_mut().get_mut::<Position>(car).unwrap().0 = Vec3::new(170.0, 0.5, COURSE_Z);
    run(&mut app, 4);
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert_eq!(progress.cleared_count(), 3, "the sweep earned the gates");
    assert_eq!(progress.state, ParticipantState::Racing);

    for e in [vpt, heavy] {
        let driver = app.world().get::<OpponentDriver>(e).unwrap();
        let policy = driver.catch_up_policy;
        assert!(
            driver.catch_up > 0.0 && driver.catch_up <= policy.assist_max,
            "a trailing driver's bounded assist: {}",
            driver.catch_up
        );
        assert_eq!(driver.reanchors, 0, "a demand lift is not a re-anchor");
        // The lifted ceiling reaches the real input: `band.min(cap)`
        // alone can never exceed the authored cap.
        let input = app.world().get::<VehicleInput>(e).unwrap();
        assert!(
            input.throttle > driver.tuning.throttle_cap,
            "assist lifts the authored cap {:?}: throttle {}",
            driver.tuning,
            input.throttle
        );
        // Progress is still earned through the triggers, never granted.
        assert_eq!(
            app.world().get::<RaceProgress>(e).unwrap().state,
            ParticipantState::Racing
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
                    d.tuning.throttle_cap,
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
        let driver = app.world().get::<OpponentDriver>(vpt).unwrap();
        assert_eq!(
            driver.catch_up, 0.0,
            "a lone leader earns no assist — the measured delta is authored tuning"
        );
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

/// Edge: a faster car behind a bus — a *moving* slower blocker, not a
/// parked one. The corridor commits the pass on closing speed and the
/// follower must actually get past: the position order swaps while the
/// pass aim visibly leaves the lane line, instead of queueing behind
/// the bus until the frame cap (AC03's bounded response on live
/// traffic). The bus is a roster slot on the same lane staged ahead
/// with a low authored `maxThrottle` — a bus, not a wall.
#[test]
fn a_faster_car_passes_a_slower_moving_blocker() {
    let tmp = roster_install_rows(
        "vpt race0-a-0.opp 1.0 0 50.0 0.7 1 1 1 1 0 1.0\n\
         vpheavy race0-a-1.opp 0.15 0 50.0 0.7 1 1 1 1 0 1.0\n",
        &[(
            "race0-a-1.opp",
            opp_file(&[
                [125.0, 0.0, COURSE_Z],
                [140.0, 0.0, COURSE_Z],
                [165.0, 0.0, COURSE_Z],
                [180.0, 0.0, COURSE_Z],
            ]),
        )],
    );
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let vpt = opponent_by_vehicle(&mut app, "vpt");
    let bus = opponent_by_vehicle(&mut app, "vpheavy");
    let bus_obj = app.world().get::<ObjectIdentity>(bus).unwrap().0;

    // The bus is ahead at the release — the follower has to earn the
    // position swap on track.
    run(&mut app, 240);
    let behind_at_start = app.world().get::<Position>(vpt).unwrap().0.x
        < app.world().get::<Position>(bus).unwrap().0.x;
    assert!(behind_at_start, "the bus must start ahead");

    let mut max_dev = 0.0f32;
    let mut impacts = Vec::new();
    let mut finished = false;
    for _ in 0..900 {
        app.update();
        impacts.extend(drain_impacts(&mut app));
        let v = app.world().get::<Position>(vpt).unwrap().0;
        max_dev = max_dev.max((v.z - COURSE_Z).abs());
        if matches!(
            app.world().get::<RaceProgress>(vpt).unwrap().state,
            ParticipantState::Finished { .. }
        ) {
            finished = true;
            break;
        }
    }
    assert!(finished, "the faster car never finished — stuck behind?");
    let v = app.world().get::<Position>(vpt).unwrap().0;
    let b = app.world().get::<Position>(bus).unwrap().0;
    assert!(
        v.x > b.x + 2.0,
        "the position order must swap on track: vpt {v:?} vs bus {b:?}"
    );
    assert!(
        max_dev > 0.5,
        "a real overtake leaves the lane line: max |z-{COURSE_Z}| = {max_dev}"
    );
    // A shove-through is not an overtake: the bus was never bulldozed
    // hard enough to count as the pass mechanism (minor rubs are the
    // corridor's business, a sustained push is not).
    let hard_contacts = impacts
        .iter()
        .filter(|e| e.participants.0 == bus_obj || e.participants.1 == bus_obj)
        .count();
    assert!(
        hard_contacts <= 2,
        "the pass must not be a sustained shove: {hard_contacts} bus impacts"
    );
    assert!(
        !matches!(
            app.world().get::<RaceProgress>(bus).unwrap().state,
            ParticipantState::Finished { .. }
        ),
        "the slow bus finishing first would mean no overtake happened"
    );
}

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
fn native_dense_route_keeps_the_corner_ahead_for_speed_planning() {
    use mm2_app::racing_line::{RouteCursor, plan_speed};
    let points: Vec<_> = (0..=4)
        .map(|i| [i as f32 * 5.0, 0.0, 0.0])
        .chain((1..=16).map(|i| [20.0, 0.0, i as f32 * 5.0]))
        .collect();
    let r = route(&points);
    let pos = Vec3::new(8.0, 0.0, -3.0);
    let (next, _) = route_target(&r, 1, pos);
    assert_eq!(next, 2, "dense anchors must not discard a turn 12 m ahead");
    let plan = plan_speed(
        &r,
        RouteCursor::locate(&r, next, pos).unwrap(),
        20.0,
        &CarLimits::of(&VehicleConfig::default(), None),
    );
    assert!(plan.limit.is_finite() && plan.limit < 20.0);
    let (next, _) = route_target(&r, 4, Vec3::new(18.0, 0.0, 3.0));
    assert_eq!(next, 5, "rounded physical corners follow the outgoing leg");
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
    assert_eq!(driver.recovery.escapes, 0);
    assert!(
        worst_lateral < 4.5,
        "dense line lost at a corner: {worst_lateral}"
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
        let mut d = app.world_mut().get_mut::<OpponentDriver>(vpt).unwrap();
        d.next = 7;
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
    run(&mut app, 1800);
    assert!(
        matches!(
            app.world().get::<RaceProgress>(vpt).unwrap().state,
            ParticipantState::Finished { .. }
        ),
        "the actual AI resumes physical traversal to finish"
    );
}

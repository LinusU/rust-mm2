//! F15-A.2 opponent integration: an authored `[Opponent]` roster on a
//! synthetic install spawns real AI participants through the production
//! `load_session_world` → `opponent_roster` → `load_opponent` path —
//! each with its own vehicle config, `PlayerControl::Ai`, `RaceProgress`
//! and session ownership — then `opponent_drive` chases the authored
//! `.opp` routes through the same `VehicleInput` → physics →
//! `advance_race` validation the player's controls feed.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::opponents::{
    Blocker, OpponentDriver, Traffic, apply_gap_brake, nearest_blocker, opponent_drive,
    pick_pass_side, route_target, spawn_pose,
};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts, race};
use mm2_assets::Vfs;
use mm2_game::{
    EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ObjectIdentity, OpponentRoute,
    OpponentRoutePoint, OpponentSpec, ParticipantState, Player, PlayerControl, PlayerVehicle,
    RaceDefinition, RaceProgress, RaceStarted, RaceState, ResultLedger, Session, SessionConfig,
    SessionEntity, SessionMode, SessionPhase, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig, VehicleInput, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

/// The same dev-world lane `tests/event.rs` uses: gates at x=110/140/165,
/// finish at x=180, all at z=140 with a 15 m trigger radius — an
/// opponent lane at z=146 still sweeps them.
const COURSE_Z: f32 = 140.0;

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

/// Minimal `vehCarSim` tune — every required field, `mass` the only
/// variable so the `_opp` variant and the heavy car are distinguishable.
fn vehcarsim(mass: f32) -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass {mass}\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

/// One quad geometry chunk: 4 verts, 2 tris, centred on `c`.
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&6u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    for p in [
        [c[0] - hx, c[1] - hy, c[2] - hz],
        [c[0] + hx, c[1] - hy, c[2] + hz],
        [c[0] + hx, c[1] + hy, c[2] - hz],
        [c[0] - hx, c[1] + hy, c[2] + hz],
    ] {
        for v in p {
            geo.extend_from_slice(&v.to_le_bytes());
        }
        for n in [0.0f32, 1.0, 0.0] {
            geo.extend_from_slice(&n.to_le_bytes());
        }
        for uv in [0.0f32, 0.0] {
            geo.extend_from_slice(&uv.to_le_bytes());
        }
    }
    geo.extend_from_slice(&6u32.to_le_bytes()); // strip indices
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

/// A PKG3 with `body_h` plus `whl0..3` — wheels authored in place
/// (no `.mtx`, so the importer falls back to geometry centres). Front
/// wheels at -Z (MM2 forward), rear driven per `DrivetrainType 0`.
fn car_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    let chunks: &[(&str, Vec<u8>)] = &[
        ("body_h", quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6)),
        ("whl0_h", quad_geo([0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl1_h", quad_geo([-0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl2_h", quad_geo([0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("whl3_h", quad_geo([-0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
    ];
    for (name, geo) in chunks {
        d.extend_from_slice(b"FILE");
        d.push(name.len() as u8 + 1);
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        d.extend_from_slice(geo);
    }
    d
}

/// An ASCII bound box — without it `convert` falls back to a centred
/// chassis cuboid whose hull rests high enough that the wheel rays
/// never reach the ground. Underside at local ~0, like the stock cars.
fn car_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

/// One vehicle: base tune + opponent tune (unless `opp_tune` is None)
/// + model + bound.
fn write_car(d: &Path, id: &str, mass: f32, opp_mass: Option<f32>) {
    write(d, &format!("tune/vehicle/{id}.vehcarsim"), vehcarsim(mass));
    if let Some(m) = opp_mass {
        write(d, &format!("tune/vehicle/{id}_opp.vehcarsim"), vehcarsim(m));
    }
    write(d, &format!("geometry/{id}.pkg"), car_pkg());
    write(d, &format!("bound/{id}_bound.bnd"), car_bnd());
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

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// A `race/testcity/` checkpoint event wiring two opponents on parallel
/// lanes (`vpt` at z=140, `vpheavy` at z=146) plus the vehicle files
/// `load_opponent` resolves through the same VFS.
fn roster_install(extra_aimap_rows: &str, extra_files: &[(&str, String)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,2,0,0.1,0.0,1,50,1,0,0,0,2,0,0.2,0.0,1,40,1\n"),
    );
    let rows = format!(
        "vpt race0-a-0.opp 0.90 0 50.0 0.7 0 0 0 0 0 1.0\nvpheavy race0-a-1.opp 0.80 0 50.0 0.7 0 0 0 0 0 1.0\n{extra_aimap_rows}"
    );
    write(d, "race/testcity/race0.aimap", aimap_with_opponents(&rows));
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
    // `vpt` ships an authored `_opp` tune (mass 2000 vs base 1000);
    // `vpheavy` (mass 3000) has no variant → base tune applies.
    write_car(d, "vpt", 1000.0, Some(2000.0));
    write_car(d, "vpheavy", 3000.0, None);
    for (rel, contents) in extra_files {
        write(d, &format!("race/testcity/{rel}"), contents);
    }
    tmp
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The headless_smoke system set plus `opponent_drive` — the real
/// session/race drivers on a minimal app.
fn event_app(config: SessionConfig, vfs: Vfs) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

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
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
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

fn run(app: &mut App, updates: usize) {
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
                yaw_deg: 90.0,
            },
            mm2_game::RaceStart {
                position: Vec3::new(56.0, 0.0, 144.0),
                yaw_deg: 90.0,
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
    // Facing: toward the first route point ahead (+X ⇒ yaw ≈ −π/2).
    assert!(
        (yaw + std::f32::consts::FRAC_PI_2).abs() < 0.35,
        "faces the route leg, got {yaw}"
    );

    // No authored grid → the route's first point anchors the spawn.
    def.start_slots.truncate(1);
    let (pos, _) = spawn_pose(&def, 0, &spec, Vec3::new(60.0, 0.0, 140.0), 1.57);
    assert_eq!(pos, Vec3::new(70.0, 0.0, 140.0), "route row0 anchors");

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

// ---------------------------------------------------------------------------
// Production-path session tests
// ---------------------------------------------------------------------------

/// The authored roster spawns real AI participants: distinct `PlayerId`s,
/// `PlayerControl::Ai`, session ownership, `RaceProgress` and their own
/// vehicle configs — the `_opp` tune on `vpt`, the base tune on `vpheavy`.
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

    // Per-vehicle configs, not a cloned player car: `vpt` picked its
    // `_opp` tune (2000), `vpheavy` its base tune (3000).
    let vpt_mass = app.world().get::<Vehicle>(vpt).unwrap().config.mass;
    let heavy_mass = app.world().get::<Vehicle>(heavy).unwrap().config.mass;
    assert_eq!(vpt_mass, 2000.0, "_opp tuning wins for vpt");
    assert_eq!(heavy_mass, 3000.0, "no variant → base tune");

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
        assert_eq!(app.world().get::<OpponentDriver>(e).unwrap().next, 0);
    }
}

fn phase(app: &App) -> SessionPhase {
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

    let b = nearest_blocker(me, pos, fwd, reach, &all).unwrap();
    assert_eq!(b.entity, all[2].entity, "the nearer corridor car wins");
    assert!((b.gap - 12.0).abs() < 0.01);
    assert!(b.lat > 0.0, "x=+1 sits to the right of a -Z heading");
    assert_eq!(b.speed, 0.0, "parked blocker reads no closing pace");

    let all = [self_entry, all[1], all[3], all[4], all[5]];
    let b = nearest_blocker(me, pos, fwd, reach, &all).unwrap();
    assert_eq!(b.entity, all[1].entity, "next-nearest corridor car");
    assert!((b.speed - 10.0).abs() < 0.01);

    // Off the corridor entirely: nothing ahead.
    let clear = [traffic(6, [4.0, 0.0, -10.0], [0.0, 0.0, -1.0], 0.0)];
    assert!(
        nearest_blocker(me, pos, fwd, reach, &clear).is_none(),
        "a car on the next lane is not a blocker"
    );
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

//! F16-B app-wiring integration: authoritative results → profile
//! progression through the real `record_session_results` system —
//! finishes record on the bound profile, authored rewards grant once,
//! and ineligible sessions (sandbox, dev world, mods, dev overrides,
//! non-local participants, timeouts) never progress.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::ImpactFilter;
use mm2_app::profile::{ActiveProfile, ProfileRequest};
use mm2_app::progression::{self, EventRewards};
use mm2_app::race;
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_assets::Vfs;
use mm2_game::{
    BangerPool, BangerStateChanged, Difficulty, EventKey, EventRef, EventTableKind, ImpactEvent,
    Mm2Vfs, Player, PlayerId, PlayerVehicle, ProfileKind, ProfileStore, RaceStarted, ResultLedger,
    Session, SessionConfig, SessionMode, SessionOutcome, SessionPhase, SessionResult,
    VehicleSelection, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin};

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

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const REWARDS: &str = "RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),\n";

/// The course: a straight +X lane at z=140 on the synthetic city road
/// (same lane the dev-world event tests use).
const COURSE: &[f32] = &[60.0, 110.0, 140.0, 165.0, 180.0];
const COURSE_Z: f32 = 140.0;

/// Minimal `vehCarSim` tune (same shape `tests/profile.rs` uses).
fn vehcarsim() -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass 1000.0\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&(-1i32).to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
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
    geo.extend_from_slice(&6u32.to_le_bytes());
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

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

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// A minimal city PSDL: one room with a two-section road running +X
/// through z≈140 — the lane the course waypoints sit on.
fn city_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    // Two road sections: sw_l, rl, rr, sw_r at x=30 and x=210 — the
    // road bed spans z∈[136,144], sidewalks out to z∈[128,152].
    let verts: &[[f32; 3]] = &[
        [30., 0., 128.],
        [30., 0., 136.],
        [30., 0., 144.],
        [30., 0., 152.],
        [210., 0., 128.],
        [210., 0., 136.],
        [210., 0., 144.],
        [210., 0., 152.],
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&2u32.to_le_bytes()); // texture table: count+1
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let attr_words: Vec<u16> = {
        let mut w = Vec::new();
        w.push(0x0a << 3);
        w.push(1); // texture ref → textures[0]
        w.push(0x00); // counted road
        w.extend_from_slice(&[2, 0, 1, 2, 3, 4, 5, 6, 7]);
        w
    };
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes());
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[30., 0., 128.]); // bounds min
    push_f32s(&mut d, &[210., 6., 152.]); // bounds max
    push_f32s(&mut d, &[120., 3., 140.]); // bounds centre
    push_f32s(&mut d, &[100.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// A synthetic install: one loadable car, one city whose road covers
/// the course, two checkpoint rows (so `race,half` needs one beaten
/// event), and a rewards table with a milestone + an indexed row.
fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "tune/vehicle/vpt.vehcarsim", vehcarsim());
    write(d, "geometry/vpt.pkg", car_pkg());
    write(d, "bound/vpt_bound.bnd", car_bnd());
    write(d, "city/testcity.psdl", city_psdl());
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!(
            "{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\n"
        ),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    write(d, "race/testcity/race1.aimap", "#\n");
    write(
        d,
        "race/testcity/race1waypoints.csv",
        format!("{WAYPOINTS}60,0,140,0,15,0,0,0,\n"),
    );
    let mut wp = WAYPOINTS.to_string();
    for x in COURSE {
        wp.push_str(&format!("{x},0,{COURSE_Z},0,15,0,0,0,\n"));
    }
    write(d, "race/testcity/race0waypoints.csv", wp);
    write(
        d,
        "race/testcity/testcity_rewards.csv",
        format!(
            "{REWARDS}race,half,vpreward,0,Half the checkpoint races beaten,\nrace,0,vppaint,3,Event zero paint,\n"
        ),
    );
    tmp
}

/// The same install with a four-row checkpoint table: `race3` sits in
/// the second, gated set (CHK-3 — the first set must be beaten first).
fn install_gated() -> tempfile::TempDir {
    let tmp = install();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!(
            "{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\n"
        ),
    );
    write(d, "race/testcity/race2.aimap", "#\n");
    write(d, "race/testcity/race3.aimap", "#\n");
    let mut wp = WAYPOINTS.to_string();
    for x in COURSE {
        wp.push_str(&format!("{x},0,{COURSE_Z},0,15,0,0,0,\n"));
    }
    write(d, "race/testcity/race3waypoints.csv", wp);
    tmp
}

fn event_config() -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/testcity.psdl".to_string(),
        },
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        vehicle: VehicleSelection {
            id: Some("vpt".to_string()),
            paint: 0,
        },
        ..SessionConfig::default()
    }
}

/// The same system set `headless_smoke` runs — the real session driver,
/// race driver and the F16-B result consumer on a minimal headless app.
fn test_app(
    config: SessionConfig,
    vfs: Vfs,
    car: SelectedCar,
    profile: Option<ActiveProfile>,
) -> App {
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
        .add_message::<BangerStateChanged>()
        .init_resource::<ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<BangerPool>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(CameraMode::Chase)
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(car)
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                mm2_app::contracts::collect_impacts,
                mm2_app::contracts::publish_vehicle_telemetry,
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
                progression::record_session_results,
            ),
        );
    if let Some(profile) = profile {
        app.insert_resource(profile);
    }
    app.finish();
    app.cleanup();
    app
}

fn bound_profile(store: &ProfileStore, kind: ProfileKind) -> ActiveProfile {
    mm2_app::profile::resolve(
        store,
        &ProfileRequest::Create {
            name: "driver".to_string(),
            rank: Difficulty::Amateur,
            kind,
        },
    )
    .unwrap()
    .expect("a created profile binds")
}

fn selected_car(dir: &Path) -> (Vfs, SelectedCar) {
    let vfs = vfs_of(dir);
    let car = SelectedCar {
        def: Some(mm2_content::load_by_id(&vfs, "vpt", 0).unwrap()),
        paint: 0,
    };
    (vfs, car)
}

fn local_player_id(app: &mut App) -> PlayerId {
    app.world_mut()
        .query_filtered::<&Player, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .expect("player vehicle")
        .id
}

/// Drive the player vehicle down the authored course to a real finish —
/// the full-throttle profile the `advance_race` path resolves into a
/// `Finished` result and the progression consumer records.
fn drive_to_finish(app: &mut App) {
    let car = app
        .world_mut()
        .query_filtered::<Entity, With<mm2_game::PlayerVehicle>>()
        .iter(app.world())
        .next()
        .unwrap();
    for f in 0..900 {
        if let Some(mut input) = app.world_mut().get_mut::<VehicleInput>(car) {
            *input = VehicleInput {
                throttle: if f > 200 { 1.0 } else { 0.0 },
                ..default()
            };
        }
        app.update();
    }
}

/// Mint and record a result for `participant` through the session's own
/// id allocator — the same identity `advance_race` produces, so the
/// consumer sees an authoritative record, not a test-shaped stand-in.
fn inject_result(app: &mut App, participant: PlayerId, outcome: SessionOutcome) {
    let tick;
    let id = {
        let mut session = app.world_mut().resource_mut::<Session>();
        tick = session.tick();
        session.mint_result_id(participant)
    };
    app.world_mut()
        .resource_mut::<ResultLedger>()
        .record(SessionResult { id, tick, outcome })
        .unwrap();
}

fn saved_progress(app: &mut App, store: &ProfileStore) -> mm2_game::ProfileProgress {
    let id = app.world().resource::<ActiveProfile>().profile.id.clone();
    store.load(&id).unwrap().profile.progress
}

/// Full production path: a bound profile drives the authored course on
/// a city world — the finish lands in the ledger, the consumer records
/// the event beaten and grants both the `race,half` milestone car and
/// the indexed `race,0` paint, persisted to disk (F16-AC02).
#[test]
fn an_authored_finish_grants_the_event_unlocks() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    ));
    assert!(
        app.world().get_resource::<EventRewards>().is_some(),
        "the event session carries its reward surface"
    );

    drive_to_finish(&mut app);

    let saved = saved_progress(&mut app, &store);
    let record = saved
        .events
        .iter()
        .find(|r| {
            r.key
                == EventKey {
                    city: "testcity".to_string(),
                    table: EventTableKind::Checkpoint,
                    stem: "race0".to_string(),
                }
        })
        .expect("the finish records the event");
    assert!(record.is_beaten());
    assert_eq!(record.best_place, Some(1), "a solo finish places first");
    assert!(record.best_race_ticks.is_some());
    assert!(
        saved.unlocks.contains("vehicle:vpreward"),
        "race,half grants"
    );
    assert!(saved.unlocks.contains("paint:vppaint:3"), "race,0 grants");
}

/// The same authored finish under a sandbox identity records nothing —
/// sandbox profiles keep selections but never progress (spec req 5).
#[test]
fn a_sandbox_profile_never_progresses() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Sandbox);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 60 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// A timed-out result is inert — no record, no grant (F16-AC03).
#[test]
fn a_timed_out_result_records_nothing() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::TimedOut { race_ticks: 60 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// Modded sessions are conservatively ineligible (designed policy —
/// DRV-6's default-conditions rule extended until per-mod impact is
/// classified).
#[test]
fn a_modded_session_records_nothing() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut config = event_config();
    config.mods_active = true;
    let mut app = test_app(config, vfs, car, Some(slot));
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 60 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// The dev world is a rig, not a recordable session — even a real
/// event's results on it are ineligible (F16-AC03).
#[test]
fn a_dev_world_event_records_nothing() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut config = event_config();
    config.world = WorldMode::DevWorld;
    let mut app = test_app(config, vfs, car, Some(slot));
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 60 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// A result minted for a different participant (an AI opponent, a
/// remote driver) never lands on the local profile.
#[test]
fn a_non_local_result_is_not_applied() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.update();

    inject_result(
        &mut app,
        PlayerId(99),
        SessionOutcome::Finished { race_ticks: 60 },
    );
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// A repeat finish across a restart is a new authoritative result — the
/// record's finish count grows but the already-held unlocks never
/// re-grant (F16-AC02 idempotency across sessions).
#[test]
fn a_repeat_finish_never_reawards() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 60 });
    app.update();
    let saved = saved_progress(&mut app, &store);
    assert_eq!(saved.unlocks.len(), 2);

    // Restart → new session generation, fresh EventRewards from the
    // reload, the persisted unlocks already held.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut generation = 0;
    for _ in 0..40 {
        app.update();
        generation = app.world().resource::<Session>().generation();
        if generation >= 2
            && matches!(
                app.world().resource::<Session>().phase(),
                SessionPhase::Countdown | SessionPhase::Playing
            )
        {
            break;
        }
    }
    assert!(generation >= 2, "restart never rebuilt the event session");
    assert!(app.world().get_resource::<EventRewards>().is_some());

    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 80 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    let record = saved.events.iter().find(|r| r.key.stem == "race0").unwrap();
    assert_eq!(record.finishes, 2, "the repeat finish still records");
    assert_eq!(
        saved.unlocks.len(),
        2,
        "held unlocks never re-grant: {:?}",
        saved.unlocks
    );
}

/// A finish earned by the scripted driver (`--bot` mounted
/// `ScriptedDrive`) is evidence-driving, not a player's result — the
/// bound profile records nothing.
#[test]
fn a_bot_driven_finish_records_nothing() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut app = test_app(event_config(), vfs, car, Some(slot));
    app.insert_resource(mm2_app::scripted::ScriptedDrive);
    app.update();

    let pid = local_player_id(&mut app);
    inject_result(&mut app, pid, SessionOutcome::Finished { race_ticks: 60 });
    app.update();

    let saved = saved_progress(&mut app, &store);
    assert!(saved.events.is_empty());
    assert!(saved.unlocks.is_empty());
}

/// A `--event` launch of a still-locked event runs — the CLI bypasses
/// the (unbuilt, F17) menu that enforces availability — but the
/// session's availability surface reports exactly which authored
/// events the bound profile has not beaten (CHK-3's set-of-three).
#[test]
fn a_locked_event_launches_with_its_gate_visible() {
    let tmp = install_gated();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound_profile(&store, ProfileKind::Standard);
    let (vfs, car) = selected_car(tmp.path());
    let mut config = event_config();
    config.mode = SessionMode::Event(EventRef {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        index: 3,
    });
    let mut app = test_app(config, vfs, car, Some(slot));
    app.update();

    // Warn-only: the locked launch still reaches the race.
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    ));
    let world = app.world();
    let rewards = world.resource::<EventRewards>();
    assert_eq!(rewards.key.stem, "race3");
    let availability = rewards
        .availability
        .of(&world.resource::<ActiveProfile>().profile, &rewards.key)
        .expect("the launched event has an availability row");
    assert!(!availability.unlocked);
    assert_eq!(
        availability
            .blocked_by
            .iter()
            .map(|k| k.stem.as_str())
            .collect::<Vec<_>>(),
        ["race0", "race1", "race2"],
        "the first checkpoint set gates the second"
    );
    // The open first-set events report unlocked, none customizable.
    let open = rewards
        .availability
        .evaluate(&world.resource::<ActiveProfile>().profile);
    assert!(open.iter().take(3).all(|a| a.unlocked && !a.customizable));
}

/// F16-B.3: the launch-time vehicle-gate note reads the same garage
/// surface F17's menu will enforce — a reward-gated car, a gated paint
/// index and a non-roster entry each report, and a sandbox identity
/// reports nothing.
#[test]
fn a_gated_vehicle_selection_reports_its_note() {
    let tmp = install();
    let d = tmp.path();
    // Give the catalog canonical `.info` rows: the test car (open),
    // the `race,half` reward car (gated), the `race,0` paint target
    // (open, one gated index), and an `.inf` leftover (unlisted).
    write(d, "tune/vpt.info", "Description=Test\nColors=A|B\n");
    write(d, "tune/vpreward.info", "Description=Reward\nColors=R\n");
    write(
        d,
        "tune/vppaint.info",
        "Description=Painted\nColors=P0|P1|P2|P3\n",
    );
    write(d, "tune/vprover.inf", "Description=Rover\nColors=Primer\n");
    let vfs = vfs_of(d);

    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut slot = bound_profile(&store, ProfileKind::Standard);
    let profile = &mut slot.profile;

    use mm2_app::profile::{VehicleGateNote, vehicle_gate_note};
    assert_eq!(
        vehicle_gate_note(&vfs, profile, "vpreward", 0),
        Some(VehicleGateNote::Locked)
    );
    assert_eq!(
        vehicle_gate_note(&vfs, profile, "vppaint", 3),
        Some(VehicleGateNote::LockedPaint)
    );
    assert_eq!(vehicle_gate_note(&vfs, profile, "vppaint", 0), None);
    assert_eq!(vehicle_gate_note(&vfs, profile, "vpt", 0), None);
    assert_eq!(
        vehicle_gate_note(&vfs, profile, "vprover", 0),
        Some(VehicleGateNote::Unlisted)
    );
    assert_eq!(
        vehicle_gate_note(&vfs, profile, "vpzzz", 0),
        Some(VehicleGateNote::Uncatalogued)
    );

    // The earned grants open exactly their targets.
    profile
        .progress
        .unlocks
        .insert("vehicle:vpreward".to_string());
    profile
        .progress
        .unlocks
        .insert("paint:vppaint:3".to_string());
    assert_eq!(vehicle_gate_note(&vfs, profile, "vpreward", 0), None);
    assert_eq!(vehicle_gate_note(&vfs, profile, "vppaint", 3), None);

    // A sandbox identity selects anything — the note never fires.
    let sandbox = bound_profile(&store, ProfileKind::Sandbox);
    assert_eq!(
        vehicle_gate_note(&vfs, &sandbox.profile, "vpreward", 0),
        None
    );
    assert_eq!(
        vehicle_gate_note(&vfs, &sandbox.profile, "vppaint", 3),
        None
    );
}

/// F16-AC01: two drivers share one install. A's finish persists on A's
/// file across a restart — a fresh `ProfileStore` on the same directory
/// is a new process's only view — and none of it leaks into B: B binds
/// with no progress, no unlocks and no remembered selections, and the
/// reward A earned still gates for B. B's own finish then lands on B
/// alone; A's file is untouched (spec req 6's no-leak rule).
#[test]
fn two_profiles_isolate_progress_across_a_restart() {
    let tmp = install();
    let d = tmp.path();
    // Catalog metadata for the reward targets so the per-profile gate
    // difference is observable through the production garage surface.
    write(d, "tune/vpreward.info", "Description=Reward\nColors=R\n");
    write(
        d,
        "tune/vppaint.info",
        "Description=Painted\nColors=P0|P1|P2|P3\n",
    );

    let store_dir = tempfile::tempdir().unwrap();
    let (a_id, b_id) = {
        let store = ProfileStore::open(store_dir.path()).unwrap();
        let a = bound_profile(&store, ProfileKind::Standard);
        let a_id = a.profile.id.clone();
        // B exists before A's run — isolation is between live profiles,
        // not creation order.
        let b_id = store
            .create("driver", Difficulty::Amateur, ProfileKind::Standard)
            .unwrap()
            .id;

        // Run 1: A binds and drives the authored course to a real
        // finish through the production race/progression path.
        let (vfs, car) = selected_car(d);
        let mut app = test_app(event_config(), vfs, car, Some(a));
        app.update();
        drive_to_finish(&mut app);
        let saved = saved_progress(&mut app, &store);
        assert!(
            saved
                .events
                .iter()
                .any(|r| r.key.stem == "race0" && r.is_beaten()),
            "A's finish records the beaten event"
        );
        assert_eq!(saved.unlocks.len(), 2, "A earns both authored grants");
        (a_id, b_id)
    };

    // The restart: the app and the store handle are gone; a reopened
    // store sees only what the files hold.
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let a_saved = store.load(&a_id).unwrap().profile;
    assert_eq!(a_saved.progress.unlocks.len(), 2);
    assert_eq!(
        a_saved.selections.vehicle,
        Some(mm2_game::VehicleChoice {
            id: "vpt".to_string(),
            paint: 0
        }),
        "A's session persisted its driven vehicle"
    );

    // Run 2: B binds fresh. No progress, no unlocks, and none of A's
    // remembered selections carried over.
    let b = mm2_app::profile::resolve(&store, &ProfileRequest::Select(b_id.as_str().to_string()))
        .unwrap()
        .expect("B binds");
    assert!(b.profile.progress.events.is_empty());
    assert!(b.profile.progress.unlocks.is_empty());
    assert_eq!(
        b.profile.selections,
        mm2_game::ProfileSelections::default(),
        "B must not inherit A's remembered selections"
    );
    // The grant A earned opens the gate for A only.
    {
        use mm2_app::profile::{VehicleGateNote, vehicle_gate_note};
        let vfs = vfs_of(d);
        assert_eq!(
            vehicle_gate_note(&vfs, &b.profile, "vpreward", 0),
            Some(VehicleGateNote::Locked),
            "A's earned car stays gated for B"
        );
        assert_eq!(vehicle_gate_note(&vfs, &a_saved, "vpreward", 0), None);
    }

    // Run 3: B drives the same event to its own finish — the record
    // and grants land on B alone.
    let (vfs, car) = selected_car(d);
    let mut app = test_app(event_config(), vfs, car, Some(b));
    app.update();
    drive_to_finish(&mut app);

    let store = ProfileStore::open(store_dir.path()).unwrap();
    let b_saved = store.load(&b_id).unwrap().profile;
    assert!(
        b_saved
            .progress
            .events
            .iter()
            .any(|r| r.key.stem == "race0" && r.is_beaten()),
        "B's own finish records on B"
    );
    assert_eq!(
        b_saved.progress.unlocks.len(),
        2,
        "B earns the same grants independently"
    );
    let a_after = store.load(&a_id).unwrap().profile;
    assert_eq!(
        a_after.progress, a_saved.progress,
        "B's session must not touch A's progress"
    );
    assert_eq!(
        a_after.selections, a_saved.selections,
        "B's session must not touch A's selections"
    );
}

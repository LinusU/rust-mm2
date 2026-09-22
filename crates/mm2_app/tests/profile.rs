//! F16-A app-wiring integration: profile binding (`--profile` /
//! `--new-profile` / the `active` marker), remembered-selection restore,
//! and selection persistence through the production
//! `load_session_world` path on a synthetic install.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::ImpactFilter;
use mm2_app::profile::{
    self, ActiveProfile, ProfileBindError, ProfileRequest, VehicleSource, choose_launch,
};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_assets::Vfs;
use mm2_game::{
    BangerPool, Difficulty, EventKey, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ProfileKind,
    ProfileStore, Session, SessionConfig, SessionMode, SessionPhase, VehicleChoice,
    advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

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

/// Minimal `vehCarSim` tune — every required field (same shape the
/// opponent tests use).
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

/// One quad geometry chunk: 4 verts, 2 tris, centred on `c`.
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
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

/// An ASCII bound box — without it `convert` falls back to a centred
/// chassis cuboid whose hull rests high enough that the wheel rays
/// never reach the ground.
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

/// A synthetic install with one loadable car (`vpt`) and one checkpoint
/// event (`race/testcity` row 0 → stem `race0`).
fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "tune/vehicle/vpt.vehcarsim", vehcarsim());
    write(d, "geometry/vpt.pkg", car_pkg());
    write(d, "bound/vpt_bound.bnd", car_bnd());
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,2,0,0.1,0.0,1,50,1,0,0,0,2,0,0.2,0.0,1,40,1\n"),
    );
    write(d, "race/testcity/race0.aimap", "[Opponent]\n0\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
            "{WAYPOINTS}60,0,140,0,15,0,0,0,\n110,0,140,0,15,0,0,0,\n140,0,140,0,15,0,0,0,\n165,0,140,0,15,0,0,0,\n180,0,140,0,15,0,0,0,\n"
        ),
    );
    tmp
}

/// An event session on the dev world — the event resolves through the
/// VFS regardless of which world hosts it, and no city geometry is
/// needed for the profile assertions.
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

/// A headless app wired like the binary's session path, minus the
/// window — the same shape `tests/session.rs` uses, plus a bound
/// `ActiveProfile` when one is supplied.
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
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(car)
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(CameraMode::Chase)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<BangerPool>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
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
            ),
        );
    if let Some(profile) = profile {
        app.insert_resource(profile);
    }
    app.finish();
    app.cleanup();
    app
}

fn bound(store: &ProfileStore, request: ProfileRequest) -> ActiveProfile {
    profile::resolve(store, &request)
        .expect("resolve failed")
        .expect("expected a bound profile")
}

/// `--profile driver-<n>` selects an existing profile and makes it the
/// store's `active` marker for later runs.
#[test]
fn select_by_id_binds_and_marks_active() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let bob = store
        .create("bob", Difficulty::Professional, ProfileKind::Standard)
        .unwrap();

    let slot = bound(&store, ProfileRequest::Select(bob.id.as_str().to_string()));
    assert_eq!(slot.profile.id, bob.id);
    assert_eq!(slot.profile.name, "bob");
    assert_eq!(store.active().unwrap().as_ref(), Some(&bob.id));
}

/// A unique display name resolves; a duplicate name is an ambiguity
/// error naming the candidate ids; an unknown selector is an error.
#[test]
fn select_by_name_resolves_only_when_unique() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    let a = store
        .create("dup", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("dup", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let c = store
        .create("solo", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();

    let slot = bound(&store, ProfileRequest::Select("solo".to_string()));
    assert_eq!(slot.profile.id, c.id);

    match profile::resolve(&store, &ProfileRequest::Select("dup".to_string())) {
        Err(ProfileBindError::Ambiguous(_, ids)) => {
            assert!(ids.contains(&a.id) && ids.contains(&b.id));
        }
        Err(e) => panic!("expected ambiguous, got {e}"),
        Ok(_) => panic!("expected ambiguous, got a bound profile"),
    }
    assert!(matches!(
        profile::resolve(&store, &ProfileRequest::Select("nobody".to_string())),
        Err(ProfileBindError::Unknown(_))
    ));
}

/// `--new-profile` creates and binds in one step; the file exists, the
/// marker is set, and the rank/kind arguments land on the profile.
#[test]
fn create_request_allocates_and_marks_active() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    let slot = bound(
        &store,
        ProfileRequest::Create {
            name: "dev".to_string(),
            rank: Difficulty::Professional,
            kind: ProfileKind::Sandbox,
        },
    );
    assert_eq!(slot.profile.name, "dev");
    assert_eq!(slot.profile.rank, Difficulty::Professional);
    assert_eq!(slot.profile.kind, ProfileKind::Sandbox);
    assert!(!slot.profile.records_progress());
    assert_eq!(store.active().unwrap().as_ref(), Some(&slot.profile.id));
    assert!(tmp.path().join("driver-0.json").exists());
}

/// With no explicit selector the store's `active` marker binds; an
/// empty store binds nothing instead of inventing an identity.
#[test]
fn implicit_bind_uses_the_active_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    assert!(
        profile::resolve(&store, &ProfileRequest::Active)
            .unwrap()
            .is_none(),
        "an empty store must bind nothing"
    );

    let first = store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store
        .create("bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.set_active(&first.id).unwrap();
    let slot = bound(&store, ProfileRequest::Active);
    assert_eq!(slot.profile.id, first.id);
}

/// An implicit bind must not gate the game on a damaged file: an
/// `active` marker pointing at an unreadable profile degrades to a
/// profile-less run and leaves the files untouched.
#[test]
fn corrupt_active_marker_degrades_to_profile_less() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    let a = store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store
        .create("bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.set_active(&a.id).unwrap();
    // Destroy every copy — nothing recoverable survives.
    std::fs::write(tmp.path().join("driver-0.json"), b"not json").unwrap();
    std::fs::write(tmp.path().join("driver-0.json.bak"), b"not json").unwrap();

    assert!(
        profile::resolve(&store, &ProfileRequest::Active)
            .unwrap()
            .is_none()
    );
    assert!(tmp.path().join("driver-0.json").exists());
}

/// Binding a profile whose main file was destroyed (a surviving `.bak`
/// wins recovery) heals the main file once at bind.
#[test]
fn recovered_backup_heals_the_main_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    let mut p = store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    p.selections.vehicle = Some(VehicleChoice {
        id: "vpbug".to_string(),
        paint: 0,
    });
    store.save(&mut p).unwrap();
    // After the second save the .bak still holds revision 1 (name only,
    // no vehicle) — destroying the main file must recover that copy.
    std::fs::write(tmp.path().join("driver-0.json"), b"torn write").unwrap();

    let slot = bound(&store, ProfileRequest::Select("driver-0".to_string()));
    assert!(slot.recovered_from_backup);
    assert_eq!(slot.profile.name, "anna");

    // The heal already ran: the main file parses again and a fresh
    // load no longer reports a recovery.
    let reload = store.load(&slot.profile.id).unwrap();
    assert!(!reload.recovered_from_backup);
    assert_eq!(reload.profile.name, "anna");
}

/// The interrupted-write leg of F16-AC04 through the app: a crash
/// between the flushed `.tmp` and its rename leaves a *complete,
/// newer* document orphaned beside the older main. `resolve` must
/// recover that copy — it is the freshest authoritative state — and
/// heal the main file at bind, same as the `.bak` path.
#[test]
fn an_interrupted_save_recovers_through_the_bind() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    let mut p = store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.save(&mut p).unwrap(); // main holds revision 2, .bak revision 1

    // Simulate the crash window: a third save flushed its `.tmp` (the
    // complete revision-3 document) but died before the rename, so the
    // main file still holds revision 2.
    let main = std::fs::read_to_string(tmp.path().join("driver-0.json")).unwrap();
    let orphaned = main.replacen("\"revision\": 2", "\"revision\": 3", 1);
    assert_ne!(main, orphaned, "the fixture must bump the revision");
    std::fs::write(tmp.path().join("driver-0.json.tmp"), orphaned).unwrap();

    let slot = bound(&store, ProfileRequest::Select("driver-0".to_string()));
    assert!(
        slot.recovered_from_backup,
        "the orphaned tmp must win over the stale main"
    );
    // The bind-time heal re-saves the recovered document, so the bound
    // profile already carries the bumped revision.
    assert_eq!(slot.profile.revision, 4, "revision 3 recovered + healed");

    // The heal already ran: a fresh load reads the main file cleanly.
    let reload = store.load(&slot.profile.id).unwrap();
    assert!(!reload.recovered_from_backup);
    assert_eq!(reload.profile.revision, 4);
    // The stale main's content is gone for good — the pre-crash
    // document was superseded, not preserved as truth.
    assert_eq!(reload.profile.name, "anna");
}

/// `choose_launch` precedence: `--car`/`--paint`/`--pro` beat the
/// remembered selections; without them the profile's vehicle, paint and
/// rank apply.
#[test]
fn choose_launch_merges_flags_over_remembered_selections() {
    let mut p = store_profile("anna", Difficulty::Professional);
    p.selections.vehicle = Some(VehicleChoice {
        id: "vpddbus".to_string(),
        paint: 2,
    });

    let (src, paint, diff) = choose_launch(None, None, false, Some(&p));
    assert_eq!(
        src,
        VehicleSource::Remembered(&VehicleChoice {
            id: "vpddbus".to_string(),
            paint: 2
        })
    );
    assert_eq!(paint, 2);
    assert_eq!(diff, Difficulty::Professional);

    // Explicit flags win outright.
    let (src, paint, diff) = choose_launch(Some("vpbug"), Some(0), true, Some(&p));
    assert_eq!(src, VehicleSource::Explicit("vpbug"));
    assert_eq!(paint, 0);
    assert_eq!(diff, Difficulty::Professional);

    // --pro overrides the profile rank; the stored rank never changes.
    let (_, _, diff) = choose_launch(None, None, false, Some(&p));
    assert_eq!(diff, Difficulty::Professional);
    let amateur = store_profile("kid", Difficulty::Amateur);
    let (_, _, diff) = choose_launch(None, None, true, Some(&amateur));
    assert_eq!(diff, Difficulty::Professional);

    // No profile: the stock default path with flag values only.
    let (src, paint, diff) = choose_launch(None, None, false, None);
    assert_eq!(src, VehicleSource::Default);
    assert_eq!(paint, 0);
    assert_eq!(diff, Difficulty::Amateur);
}

fn store_profile(name: &str, rank: Difficulty) -> mm2_game::PlayerProfile {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(tmp.path()).unwrap();
    store.create(name, rank, ProfileKind::Standard).unwrap()
}

/// The persist leg: a live event session records the driven vehicle and
/// the event's stem-keyed identity on the bound profile — through
/// `load_session_world`, the same system the binary runs.
#[test]
fn session_start_persists_selections() {
    let install = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound(
        &store,
        ProfileRequest::Create {
            name: "anna".to_string(),
            rank: Difficulty::Amateur,
            kind: ProfileKind::Standard,
        },
    );

    let vfs = vfs_of(install.path());
    let def = mm2_content::load_by_id(&vfs, "vpt", 0).unwrap();
    let mut app = test_app(
        event_config(),
        vfs,
        SelectedCar {
            def: Some(def),
            paint: 0,
        },
        Some(slot),
    );
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    ));

    let saved = store.load(&profile_id(&mut app)).unwrap().profile;
    assert_eq!(
        saved.selections.vehicle,
        Some(VehicleChoice {
            id: "vpt".to_string(),
            paint: 0
        })
    );
    assert_eq!(
        saved.selections.last_event,
        Some(EventKey {
            city: "testcity".to_string(),
            table: EventTableKind::Checkpoint,
            stem: "race0".to_string(),
        }),
        "the event must record its stable stem, not a row index"
    );
}

fn profile_id(app: &mut App) -> mm2_game::ProfileId {
    app.world().resource::<ActiveProfile>().profile.id.clone()
}

/// A dev-car cruise leaves the remembered vehicle and last event alone
/// — selections only move when a real choice ran.
#[test]
fn cruise_without_a_car_keeps_remembered_selections() {
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut slot = bound(
        &store,
        ProfileRequest::Create {
            name: "anna".to_string(),
            rank: Difficulty::Amateur,
            kind: ProfileKind::Standard,
        },
    );
    slot.profile.selections.vehicle = Some(VehicleChoice {
        id: "vpddbus".to_string(),
        paint: 1,
    });
    slot.profile.selections.last_event = Some(EventKey {
        city: "sf".to_string(),
        table: EventTableKind::Circuit,
        stem: "circuit3".to_string(),
    });

    let mut app = test_app(
        SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        Some(slot),
    );
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    let saved = store
        .load(&profile_id(&mut app))
        .unwrap()
        .profile
        .selections;
    assert_eq!(
        saved.vehicle,
        Some(VehicleChoice {
            id: "vpddbus".to_string(),
            paint: 1
        }),
        "a dev-car session must not erase the remembered vehicle"
    );
    assert_eq!(
        saved.last_event,
        Some(EventKey {
            city: "sf".to_string(),
            table: EventTableKind::Circuit,
            stem: "circuit3".to_string(),
        }),
        "a cruise must not erase the last played event"
    );
}

/// Isolation at the app level: a session bound to one profile writes
/// only that profile's file — the sibling's bytes are untouched.
#[test]
fn a_session_writes_only_the_bound_profile() {
    let install = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    store
        .create("anna", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let bob = store
        .create("bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let anna_before = std::fs::read(store_dir.path().join("driver-0.json")).unwrap();

    let vfs = vfs_of(install.path());
    let def = mm2_content::load_by_id(&vfs, "vpt", 0).unwrap();
    let slot = bound(&store, ProfileRequest::Select(bob.id.as_str().to_string()));
    let mut app = test_app(
        event_config(),
        vfs,
        SelectedCar {
            def: Some(def),
            paint: 0,
        },
        Some(slot),
    );
    app.update();

    let anna_after = std::fs::read(store_dir.path().join("driver-0.json")).unwrap();
    assert_eq!(
        anna_before, anna_after,
        "the unselected profile's bytes must be untouched"
    );
    let anna = store.load(&mm2_game::ProfileId::from("driver-0")).unwrap();
    assert!(
        anna.profile.selections.vehicle.is_none(),
        "the unselected profile must not gain the session's selections"
    );
}

/// A failed event load records nothing: `last_event` must never point
/// at an event the player never entered.
#[test]
fn a_failed_session_records_no_event() {
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let slot = bound(
        &store,
        ProfileRequest::Create {
            name: "anna".to_string(),
            rank: Difficulty::Amateur,
            kind: ProfileKind::Standard,
        },
    );

    // The event ref names a table row that does not exist on the empty
    // install — the session lands in `Failed`.
    let mut app = test_app(
        event_config(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        Some(slot),
    );
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Failed(_)
    ));

    let saved = store.load(&profile_id(&mut app)).unwrap().profile;
    assert!(saved.selections.last_event.is_none());
}

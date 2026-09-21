//! F17-A.1 menu integration: the shell over the real catalogs —
//! navigation, launches through `Session::begin`, availability gating,
//! profile bind/create/delete (including the deliberate-confirmation
//! step, F16-AC06) and the quit-back-to-menu loop, driven through the
//! production `menu_*`/`drive_session` systems on a headless app.
//!
//! The synthetic install carries one loadable car (`vpt`, two paints
//! with `Blue` reward-gated), one city, and a four-row checkpoint
//! table so `race2` exercises the incomplete-row leg and `race3` the
//! CHK-3 gating leg.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::ImpactFilter;
use mm2_app::menu::{self, MenuCamera, MenuData, MenuShell, MenuUi};
use mm2_app::profile::ActiveProfile;
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_assets::Vfs;
use mm2_game::{
    BangerPool, Difficulty, EventTableKind, ImpactEvent, Mm2Vfs, PlayerVehicle, ProfileKind,
    ProfileStore, ResultLedger, Session, SessionMode, SessionPhase, VehicleSelection,
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
const REWARDS: &str = "RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),\n";

/// A minimal city PSDL (same two-section road the progression tests
/// use — enough for `load_session_world` to reach `Ready`).
fn city_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes());
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
        for f in v {
            d.extend_from_slice(&f.to_le_bytes());
        }
    }
    d.extend_from_slice(&3u32.to_le_bytes());
    for f in [0.15f32, 2.0, 6.0] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    d.extend_from_slice(&2u32.to_le_bytes());
    d.push("test_road".len() as u8 + 1);
    d.extend_from_slice(b"test_road");
    d.push(0);
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let attr_words: Vec<u16> = {
        let mut w = vec![0x0a << 3, 1, 0x00];
        w.extend_from_slice(&[2, 0, 1, 2, 3, 4, 5, 6, 7]);
        w
    };
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes());
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes());
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]);
    d.extend_from_slice(&[0u8; 2]);
    for f in [30f32, 0., 128.] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    for f in [210f32, 6., 152.] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    for f in [120f32, 3., 140.] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    d.extend_from_slice(&100f32.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// Minimal `vehCarSim` tune (same shape the other tests use).
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

/// 48-byte wheel transform: bounds around `origin` (front-left wheel).
fn wheel_mtx() -> Vec<u8> {
    let mut d = Vec::new();
    for f in [
        -0.15f32, -0.3, -0.15, // bounds min
        0.15, 0.3, 0.15, // bounds max
        0.0, 0.0, 0.0, // pivot
        0.8, 0.3, -1.3, // origin
    ] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    d
}

fn waypoints_csv() -> String {
    let mut wp = WAYPOINTS.to_string();
    for x in [60.0f32, 110.0, 140.0, 165.0, 180.0] {
        wp.push_str(&format!("{x},0,140,0,15,0,0,0,\n"));
    }
    wp
}

/// A synthetic install: one listed car (`vpt` — canonical `.info` with
/// two paints), one city, four checkpoint rows (`race2` deliberately
/// incomplete — no waypoints — and `race3` gated by CHK-3 on the first
/// set) and a rewards table gating `vpt`'s paint 1.
fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "tune/vpt.info",
        "Description=Test Car\nColors=Red|Blue\n",
    );
    write(d, "tune/vehicle/vpt.vehcarsim", vehcarsim());
    write(d, "geometry/vpt.pkg", car_pkg());
    write(d, "bound/vpt_bound.bnd", car_bnd());
    write(d, "geometry/vpt_whl0.mtx", wheel_mtx());
    write(d, "city/testcity.psdl", city_psdl());
    let rows = "none,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\n";
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\n{rows}{rows}{rows}{rows}"),
    );
    for stem in ["race0", "race1", "race2", "race3"] {
        write(d, &format!("race/testcity/{stem}.aimap"), "#\n");
    }
    // race2 gets no waypoints on purpose — an incomplete catalog row.
    for stem in ["race0", "race1", "race3"] {
        write(
            d,
            &format!("race/testcity/{stem}waypoints.csv"),
            waypoints_csv(),
        );
    }
    write(
        d,
        "race/testcity/testcity_rewards.csv",
        format!("{REWARDS}race,0,vpt,1,Beaten event zero paint,\n"),
    );
    tmp
}

/// A headless app wired like the binary's menu mode: parked session,
/// the menu resources and the production session/menu systems, minus
/// the window.
fn menu_app(dir: &Path, store: Option<ProfileStore>) -> App {
    let vfs = vfs_of(dir);

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
        .insert_resource(Session::new())
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ImpactFilter>()
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
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(MenuShell::new(
            VehicleSelection {
                id: Some("vpt".to_string()),
                paint: 0,
            },
            Difficulty::Amateur,
        ))
        .insert_resource(MenuData::new(store, false, None))
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
                (menu::menu_watch, menu::menu_input, menu::menu_present).chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

/// Press a key for exactly one update (no InputPlugin runs here, so
/// the input state is managed by hand). `reset_all`, not `clear` —
/// `clear` keeps `pressed`, so a repeated key would never re-fire
/// `just_pressed` and navigation would stall.
fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

fn shell(app: &App) -> &MenuShell {
    app.world().resource::<MenuShell>()
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

/// Move focus onto the row containing `needle` through the real key
/// path (Up/Down only — no direct writes).
fn focus_row(app: &mut App, needle: &str) {
    let target = shell(app)
        .rows
        .iter()
        .position(|r| r.text.contains(needle))
        .unwrap_or_else(|| {
            let rows: Vec<String> = shell(app).rows.iter().map(|r| r.text.clone()).collect();
            panic!("no row containing {needle:?} — rows: {rows:?}")
        });
    while shell(app).focus != target {
        let key = if shell(app).focus < target {
            KeyCode::ArrowDown
        } else {
            KeyCode::ArrowUp
        };
        press(app, key);
    }
}

/// Focus the row containing `needle` and activate it.
fn activate_row(app: &mut App, needle: &str) {
    focus_row(app, needle);
    press(app, KeyCode::Enter);
}

fn run_until(app: &mut App, max: usize, mut pred: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..max {
        app.update();
        if pred(app) {
            return true;
        }
    }
    false
}

fn menu_roots(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, (With<MenuUi>, Without<ChildOf>)>()
        .iter(world)
        .count()
}

/// Every `MenuUi` entity (root plus rows) — a leaked child would not
/// show in `menu_roots`.
fn menu_entities(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<MenuUi>>()
        .iter(world)
        .count()
}

/// The menu's own render target. `bevy_ui` draws per camera view, so a
/// shell with no `MenuCamera` is invisible even though its rows exist.
fn menu_cameras(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<MenuCamera>>()
        .iter(world)
        .count()
}

fn players(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(world)
        .count()
}

/// Boot parks at `Menu` and draws the shell — the first F17-A behavior:
/// the app no longer falls straight into a world.
#[test]
fn the_app_boots_into_the_menu() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    assert_eq!(phase(&app), SessionPhase::Menu);
    assert!(menu_roots(&mut app) >= 1, "the menu should draw");
    assert_eq!(
        menu_cameras(&mut app),
        1,
        "the menu needs a camera or nothing renders"
    );
    assert_eq!(players(&mut app), 0);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        phase(&app),
        SessionPhase::Menu,
        "the menu must not self-launch"
    );
    // The root rows include the unavailable legs, visible with reasons.
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert!(texts.iter().any(|t| t == "Cruise"));
    assert!(texts.iter().any(|t| t.starts_with("Vehicle:")));
    assert!(texts.iter().any(|t| t == "Quit"));
    assert!(
        shell(&app)
            .rows
            .iter()
            .any(|r| { r.enabled.is_err() && (r.text == "Options" || r.text == "Multiplayer") })
    );
}

/// Regression for the F17-A.1 review blocker: `bevy_ui` renders per
/// camera view and the only cameras in the app are session-owned, so a
/// menu without its own camera built an invisible tree. The shell must
/// own a live `Camera2d`, pin the UI roots to it, keep it stable across
/// redraws, and drop it when a session takes the screen.
#[test]
fn the_menu_draws_into_its_own_camera() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    let (cam, active) = {
        let world = app.world_mut();
        let mut cams = world.query_filtered::<(Entity, &Camera), With<MenuCamera>>();
        let (entity, camera) = cams
            .iter(world)
            .next()
            .expect("the menu must spawn a camera to draw into");
        (entity, camera.is_active)
    };
    assert!(active, "a disabled camera renders nothing");
    {
        let world = app.world_mut();
        let mut ui = world.query_filtered::<&UiTargetCamera, (With<MenuUi>, Without<ChildOf>)>();
        let targets: Vec<Entity> = ui.iter(world).map(|t| t.0).collect();
        assert_eq!(
            targets,
            vec![cam],
            "the menu UI root must be pinned to the menu camera"
        );
    }

    // A redraw (focus moved, tree rebuilt) keeps the same camera
    // entity — presentation churn must not churn the render target.
    press(&mut app, KeyCode::ArrowDown);
    {
        let world = app.world_mut();
        let mut cams = world.query_filtered::<Entity, With<MenuCamera>>();
        let after: Vec<Entity> = cams.iter(world).collect();
        assert_eq!(after, vec![cam], "a redraw must not churn the camera");
    }

    // Launch: the session supplies its own cameras and the menu's is
    // gone — the shell never double-draws over a live world.
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
    assert_eq!(menu_cameras(&mut app), 0);
    {
        let world = app.world_mut();
        let mut cams = world.query_filtered::<Entity, (With<Camera>, Without<MenuCamera>)>();
        assert!(
            cams.iter(world).count() >= 1,
            "the session supplies its own cameras"
        );
    }

    // Quit-to-menu brings the render target back — a returning shell
    // is drawable again, not just active.
    press(&mut app, KeyCode::Escape);
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu));
    app.update();
    assert_eq!(menu_cameras(&mut app), 1);
}

/// Cruise → city → session: the whole launch leg through the real
/// `Session::begin`, then `Esc` quits back to the menu and a second
/// cycle leaves exactly one of everything (AC06's menu leg).
#[test]
fn cruise_launches_then_quit_returns_to_the_menu() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(
        run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing),
        "the cruise never reached Playing: {:?}",
        phase(&app)
    );
    assert_eq!(players(&mut app), 1);
    assert_eq!(menu_roots(&mut app), 0, "the menu should hide in-game");
    assert_eq!(
        menu_entities(&mut app),
        0,
        "no menu row leaks into the session"
    );
    assert_eq!(
        menu_cameras(&mut app),
        0,
        "the menu camera must not outlive the menu (AC06 — no duplicate cameras)"
    );
    let car = app.world().resource::<SelectedCar>();
    assert_eq!(car.def.as_ref().map(|d| d.id.as_str()), Some("vpt"));

    // Esc in the world is quit-to-menu, not quit-to-exit.
    press(&mut app, KeyCode::Escape);
    assert!(
        run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu),
        "quit never reached Menu"
    );
    app.update();
    assert!(shell(&app).active, "the menu should reopen");
    assert_eq!(menu_roots(&mut app), 1, "exactly one menu root");
    assert_eq!(menu_cameras(&mut app), 1, "the menu camera is back");
    assert_eq!(players(&mut app), 0);
    assert!(
        app.world().resource::<Messages<AppExit>>().is_empty(),
        "quit-to-menu must not write AppExit"
    );

    // A second cycle stays clean — one menu root, one player.
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
    assert_eq!(players(&mut app), 1);
    press(&mut app, KeyCode::Escape);
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu));
    app.update();
    assert_eq!(menu_roots(&mut app), 1);

    // Esc at the root is the menu's exit path.
    press(&mut app, KeyCode::Escape);
    let exits: Vec<AppExit> = app
        .world_mut()
        .resource_mut::<Messages<AppExit>>()
        .drain()
        .collect();
    assert!(
        exits.iter().any(|e| matches!(e, AppExit::Success)),
        "Esc at the root should exit, got {exits:?}"
    );
}

/// The Events path enforces availability: a gated row is listed but
/// refuses activation with its reason, an incomplete row reports its
/// missing files, and an open row launches the real `EventRef`.
#[test]
fn event_rows_carry_real_availability() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    // Table screen: Checkpoint has 4 rows; the empty tables and the
    // still-unsupported Crash Course stay visible with reasons.
    {
        let rows = &shell(&app).rows;
        let crash = rows
            .iter()
            .find(|r| r.text.starts_with("Crash Course"))
            .unwrap();
        assert!(crash.enabled.is_err());
        let blitz = rows.iter().find(|r| r.text.starts_with("Blitz")).unwrap();
        assert!(blitz.enabled.is_err());
    }
    activate_row(&mut app, "Checkpoint");

    // The event list: race0/race1 open, race2 incomplete, race3 gated.
    let enabled: Vec<(String, Result<(), String>)> = shell(&app)
        .rows
        .iter()
        .map(|r| (r.text.clone(), r.enabled.clone()))
        .collect();
    assert_eq!(enabled.len(), 4);
    assert!(enabled[0].0.contains("race0") && enabled[0].1.is_ok());
    assert!(enabled[1].0.contains("race1") && enabled[1].1.is_ok());
    assert!(enabled[2].0.contains("race2"));
    assert!(
        enabled[2].1.as_ref().unwrap_err().contains("incomplete"),
        "race2 should report its missing files: {:?}",
        enabled[2].1
    );
    assert!(enabled[3].0.contains("race3"));
    assert!(
        enabled[3].1.as_ref().unwrap_err().contains("race0"),
        "race3 should name its gate: {:?}",
        enabled[3].1
    );

    // Activating the gated row is a status line, never a launch.
    activate_row(&mut app, "race3");
    assert_eq!(phase(&app), SessionPhase::Menu);
    assert!(
        shell(&app).status.as_deref().unwrap_or("").contains("beat"),
        "the gate reason should land on the status line"
    );

    // The open row launches the real event through `Session::begin`.
    activate_row(&mut app, "race0");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "race0 never launched: {:?}",
        phase(&app)
    );
    match app.world().resource::<Session>().config() {
        Some(cfg) => assert_eq!(
            cfg.mode,
            SessionMode::Event(mm2_game::EventRef {
                city: "testcity".into(),
                table: EventTableKind::Checkpoint,
                index: 0,
            })
        ),
        None => panic!("a launched session has a config"),
    }
}

/// Garage → paints → launch: the roster row is the real catalog entry,
/// a reward-gated paint stays disabled with its reason, and the pick
/// lands in the launched session's `SelectedCar`.
#[test]
fn garage_picks_carry_through_launch() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    activate_row(&mut app, "Vehicle:");
    {
        let rows = &shell(&app).rows;
        let car = rows.iter().find(|r| r.text.contains("Test Car")).unwrap();
        assert!(car.enabled.is_ok(), "vpt should be selectable");
    }
    activate_row(&mut app, "Test Car");
    // Picking the car opened its paint list; paint 1 is reward-gated.
    let rows: Vec<(String, Result<(), String>)> = shell(&app)
        .rows
        .iter()
        .map(|r| (r.text.clone(), r.enabled.clone()))
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].0.contains("Red") && rows[0].1.is_ok());
    assert!(rows[1].0.contains("Blue"));
    assert!(
        rows[1].1.as_ref().unwrap_err().contains("locked"),
        "Blue should be reward-gated: {:?}",
        rows[1].1
    );
    activate_row(&mut app, "Blue");
    assert_eq!(
        shell(&app).vehicle.paint,
        0,
        "a locked paint cannot be selected"
    );
    activate_row(&mut app, "Red");
    assert_eq!(shell(&app).vehicle.paint, 0);

    // Back out to the root and launch — the pick drives the session.
    // (PickPaint already popped Paints back to the Garage.)
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
    let car = app.world().resource::<SelectedCar>();
    assert_eq!(car.def.as_ref().map(|d| d.id.as_str()), Some("vpt"));
}

/// The Driver screen binds, creates and deletes real profiles through
/// `ProfileStore` — with the delete sitting behind its own
/// confirmation screen (F16-AC06's deliberate-confirmation leg) and
/// the last-profile refusal surfaced as a status line (DRV-7).
#[test]
fn profiles_bind_create_and_delete() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let bob = store
        .create("Bob", Difficulty::Professional, ProfileKind::Standard)
        .unwrap();
    let carol = store
        .create("Carol", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();

    let mut app = menu_app(tmp.path(), Some(store.clone()));
    app.update();

    activate_row(&mut app, "Driver:");
    let labels: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert!(labels.iter().any(|t| t.contains("Alice")));
    assert!(labels.iter().any(|t| t.contains("Bob")));
    assert!(labels.iter().any(|t| t.contains("Carol")));

    // Binding a profile marks it active, inserts the resource and
    // seeds the difficulty from its rank (DRV-2).
    activate_row(&mut app, "Bob");
    assert_eq!(
        store.active().unwrap().as_ref().map(|id| id.as_str()),
        Some(bob.id.as_str()),
    );
    assert_eq!(shell(&app).difficulty, Difficulty::Professional);
    app.update();
    assert!(
        app.world().get_resource::<ActiveProfile>().is_some(),
        "the bind effect should land the resource"
    );

    // X on Alice opens the confirmation screen — a plain activation of
    // the row would have bound her instead; deletion is never the
    // default action.
    focus_row(&mut app, "Alice");
    press(&mut app, KeyCode::KeyX);
    assert!(
        matches!(shell(&app).screen, menu::Screen::ConfirmDelete { .. }),
        "X should open the delete confirmation"
    );
    // Still not deleted — the row moved, the profile remains.
    store.load(&alice.id).unwrap();
    // Confirming deletes for real.
    activate_row(&mut app, "Delete");
    assert!(
        store.load(&alice.id).is_err(),
        "the confirmed delete removes the profile"
    );

    // Deleting the *bound* profile unbinds it — Carol remains, so the
    // store allows it.
    focus_row(&mut app, "Bob");
    press(&mut app, KeyCode::KeyX);
    activate_row(&mut app, "Delete");
    assert!(store.load(&bob.id).is_err());
    app.update();
    assert!(
        app.world().get_resource::<ActiveProfile>().is_none(),
        "deleting the bound profile must unbind it"
    );

    // A fresh driver binds immediately.
    activate_row(&mut app, "New driver");
    app.update();
    let created = app
        .world()
        .get_resource::<ActiveProfile>()
        .expect("create binds the new profile")
        .profile
        .id
        .clone();

    // Carol (not bound, not last) still deletes. (We're still on the
    // Profiles screen — create doesn't navigate.)
    focus_row(&mut app, "Carol");
    press(&mut app, KeyCode::KeyX);
    activate_row(&mut app, "Delete");
    assert!(store.load(&carol.id).is_err());

    // DRV-7: the store refuses to delete its last profile — the menu
    // surfaces the refusal instead of claiming success, and the bound
    // profile survives.
    focus_row(&mut app, created.as_str());
    press(&mut app, KeyCode::KeyX);
    activate_row(&mut app, "Delete");
    assert!(
        store.load(&created).is_ok(),
        "the last profile survives a confirmed delete"
    );
    assert!(
        shell(&app).status.as_deref().unwrap_or("").contains("last"),
        "the refusal reason lands on the status line"
    );
    assert!(
        app.world().get_resource::<ActiveProfile>().is_some(),
        "the refused delete leaves the profile bound"
    );
}

/// No content and no store: the rows exist but carry honest reasons —
/// nothing silently hides and nothing launches.
#[test]
fn an_empty_install_reports_instead_of_faking() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    let rows: Vec<(String, Result<(), String>)> = shell(&app)
        .rows
        .iter()
        .map(|r| (r.text.clone(), r.enabled.clone()))
        .collect();
    let find = |needle: &str| rows.iter().find(|(t, _)| t.contains(needle));
    assert!(
        find("Cruise")
            .unwrap()
            .1
            .as_ref()
            .unwrap_err()
            .contains("--mm2-path")
    );
    assert!(find("Vehicle:").unwrap().1.is_err());
    assert!(find("Driver:").unwrap().1.is_err());

    // Events is enterable but every city row explains the missing psdl.
    activate_row(&mut app, "Events");
    let cities: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert!(
        cities.iter().any(|c| c.contains("london")) && cities.iter().any(|c| c.contains("sf")),
        "expected cities stay listed: {cities:?}"
    );
    assert!(
        shell(&app).rows.iter().all(|r| r.enabled.is_err()),
        "every city should report its missing data"
    );
    activate_row(&mut app, "london");
    assert_eq!(phase(&app), SessionPhase::Menu, "nothing can launch");
    assert!(shell(&app).status.is_some());
}

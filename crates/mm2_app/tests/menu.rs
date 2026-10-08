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
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey, NativeKeyCode};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::window::PrimaryWindow;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::ImpactFilter;
use mm2_app::controls::{ControlSettings, DriveAction};
use mm2_app::display_trial;
use mm2_app::menu::{self, MenuCamera, MenuData, MenuShell, MenuUi};
use mm2_app::pause::{self, PauseMenu};
use mm2_app::profile::ActiveProfile;
use mm2_app::results::{self, ResultsMenu};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_app::settings::{
    Antialiasing, DisplayMode, FieldOfView, GraphicsSettings, RunOverrides, ShadowQuality,
    TextSize, WindowSize, settings_path,
};
use mm2_assets::Vfs;
use mm2_game::{
    BangerPool, Difficulty, EventKey, EventTableKind, ImpactEvent, Mm2Vfs, PlayerVehicle,
    ProfileId, ProfileKind, ProfileStore, ResultLedger, Session, SessionMode, SessionPhase,
    VehicleSelection, advance_session_tick, despawn_session_entities,
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
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
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

/// The checkpoint install plus one Circuit event — `circuit0` authors
/// `Opponents` 2 / `NumLaps` 2, wires two `vpt` opponents on closed
/// `.opp` loops and ships the >= 4 waypoint rows an Ordered course
/// needs. The RACE-3 race-shape options tests drive it.
fn circuit_install() -> tempfile::TempDir {
    let tmp = install();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,2,0,0.1,0.0,2,50,1,0,0,0,2,0,0.2,0.0,2,40,1\n"),
    );
    write(
        d,
        "race/testcity/circuit0.aimap",
        "[Opponent]\n2\n\
         vpt circuit0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n\
         vpt circuit0-a-1.opp 0.80 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        format!(
            "{WAYPOINTS}60,0,140,0,15,0,0,0,\n110,0,140,0,15,0,0,0,\n140,0,140,0,15,0,0,0,\n165,0,140,0,15,0,0,0,\n"
        ),
    );
    // Closed loops down and back along the lane.
    let opp = |z: f32, back_z: f32| {
        format!(
            "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n\
             70,0,{z},0,0,0,0,0,0\n180,0,{z},0,0,0,0,0,0\n180,0,{back_z},0,0,0,0,0,0\n\
             70,0,{back_z},0,0,0,0,0,0\n72,0,{z},0,0,0,0,0,0\n"
        )
    };
    write(d, "race/testcity/circuit0-a-0.opp", opp(140.0, 146.0));
    write(d, "race/testcity/circuit0-a-1.opp", opp(146.0, 140.0));
    tmp
}

/// A headless app wired like the binary's menu mode: parked session,
/// the menu resources and the production session/menu systems, minus
/// the window.
fn menu_app(dir: &Path, store: Option<ProfileStore>) -> App {
    menu_app_with(dir, MenuData::new(store, false, None))
}

/// [`menu_app`] over a caller-built [`MenuData`] (mods, settings …).
fn menu_app_with(dir: &Path, data: MenuData) -> App {
    menu_app_vfs(vfs_of(dir), data)
}

/// [`menu_app_with`] over a caller-built VFS — an install with mods
/// mounted over it.
fn menu_app_vfs(vfs: Vfs, data: MenuData) -> App {
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
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<BangerPool>()
        .init_resource::<SessionControl>()
        .init_resource::<session::SessionNote>()
        .init_resource::<PauseMenu>()
        .init_resource::<ResultsMenu>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_message::<KeyboardInput>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(CameraMode::Chase)
        .insert_resource(SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
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
        .insert_resource(data)
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                // Same ordering contract as the binary: pause owns
                // `Paused`, running between the intent reader (which
                // ignores `Paused`) and the driver.
                pause::pause_input
                    .after(session::session_control_input)
                    .before(session::drive_session)
                    .run_if(not(display_trial::display_trial_pending)),
                // `Results` gets the same ownership contract — the
                // overlay's keys, between the intent reader and the
                // driver.
                results::results_input
                    .after(session::session_control_input)
                    .before(session::drive_session),
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
                pause::sync_physics_pause.after(session::drive_session),
                pause::pause_present.after(session::drive_session),
                results::results_present.after(session::drive_session),
                // Same chain as the binary: the mouse path queues
                // commands for `menu_input`.
                (
                    menu::menu_watch,
                    menu::menu_mouse.run_if(not(display_trial::display_trial_pending)),
                    menu::menu_input.run_if(not(display_trial::display_trial_pending)),
                    menu::menu_present,
                    menu::menu_preview_motion,
                )
                    .chain(),
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

/// Feed one `KeyboardInput` press through the real message path —
/// `menu_input` reads `ev.text`/`key_code` on the name-entry screen.
fn key_text(app: &mut App, key_code: KeyCode, text: Option<&str>) {
    app.world_mut().write_message(KeyboardInput {
        key_code,
        logical_key: text.map_or(Key::Unidentified(NativeKey::Unidentified), |t| {
            Key::Character(t.into())
        }),
        state: ButtonState::Pressed,
        text: text.map(Into::into),
        repeat: false,
        window: Entity::PLACEHOLDER,
    });
}

/// Type a string into the focused field (one message per char), then
/// run one update so `menu_input` consumes the batch.
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key_text(
            app,
            KeyCode::Unidentified(NativeKeyCode::Unidentified),
            Some(&c.to_string()),
        );
    }
    app.update();
}

/// One Backspace press on the entry field.
fn erase(app: &mut App) {
    key_text(app, KeyCode::Backspace, None);
    app.update();
}

/// The entry screen's edit buffer, or a failure elsewhere.
fn name_buffer(app: &App) -> String {
    match &app.world().resource::<MenuShell>().screen {
        menu::Screen::NewProfile { name } => name.clone(),
        other => panic!("expected the name-entry screen, on {other:?}"),
    }
}

/// The text the menu's UI tree currently draws.
fn menu_texts(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&Text, With<MenuUi>>();
    q.iter(world).map(|t| t.0.clone()).collect()
}

fn shell(app: &App) -> &MenuShell {
    app.world().resource::<MenuShell>()
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

/// Leave a live session through the pause menu. `Esc` on `Playing`
/// pauses (navigate to the Quit row); `Esc` on `Countdown` still
/// requests quit directly — the lifecycle has no `Countdown → Paused`
/// edge.
fn quit_session(app: &mut App) {
    press(app, KeyCode::Escape);
    if phase(app) == SessionPhase::Paused {
        for _ in 0..3 {
            press(app, KeyCode::ArrowDown);
        }
        press(app, KeyCode::Enter);
    }
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

/// Focus the row containing `needle`, step right onto its options
/// side entry and open it.
fn open_options(app: &mut App, needle: &str) {
    focus_row(app, needle);
    press(app, KeyCode::ArrowRight);
    assert!(shell(app).side, "{needle:?} has no options entry");
    press(app, KeyCode::Enter);
}

/// The options side entry of the row containing `needle`.
fn options_of<'a>(app: &'a App, needle: &str) -> &'a menu::Row {
    shell(app)
        .rows
        .iter()
        .find(|r| r.text.contains(needle))
        .and_then(|r| r.side.as_deref())
        .unwrap_or_else(|| panic!("no options entry beside {needle:?}"))
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

/// A primary window the mouse tests drive. `Window` is a plain
/// component — no WindowPlugin needed — and the default scale factor
/// makes logical and physical cursor coordinates coincide.
fn spawn_window(app: &mut App) {
    app.world_mut().spawn((PrimaryWindow, Window::default()));
}

/// The y `lay_out_rows` gives the row with `MenuRow::index == index`.
fn row_y(index: usize) -> f32 {
    100.0 + index as f32 * 40.0
}

/// Simulate the UI layout pass on the real `MenuRow` entities —
/// headless tests have no UiPlugin, so `ComputedNode`/`UiGlobalTransform`
/// keep their required-component defaults (a zero-size rect at the
/// origin) unless the test places them. Row `index` gets a
/// 400x22 rect centred at `(200, row_y(index))`, so cursor positions
/// map back to rows by `index`.
fn lay_out_rows(app: &mut App) {
    let world = app.world_mut();
    let mut q = world.query::<(Entity, &menu::MenuRow)>();
    let rows: Vec<(Entity, usize)> = q.iter(world).map(|(e, r)| (e, r.index)).collect();
    for (entity, index) in rows {
        world.entity_mut(entity).insert((
            ComputedNode {
                size: Vec2::new(400.0, 22.0),
                ..default()
            },
            UiGlobalTransform::from_xy(200.0, row_y(index)),
        ));
    }
}

/// Move the cursor to a logical position (== physical at scale 1).
fn cursor_to(app: &mut App, x: f32, y: f32) {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    let mut window = q.single_mut(world).expect("the test spawned a window");
    window.set_cursor_position(Some(Vec2::new(x, y)));
}

/// Press a mouse button for exactly one update — the same one-frame
/// convention `press` uses for keys.
fn click(app: &mut App, button: MouseButton) {
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(button);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .reset_all();
}

/// Write a recorded finish into a stored profile — the same
/// `EventRecord` fields `record_session_results` produces from a real
/// session, seeded so the menu reads persisted data.
fn seed_record(
    store: &ProfileStore,
    id: &ProfileId,
    key: EventKey,
    ticks: u64,
    place: Option<u32>,
) {
    let mut loaded = store.load(id).unwrap().profile;
    loaded
        .event_mut(key)
        .record_finish(ticks, place, Difficulty::Amateur);
    store.save(&mut loaded).unwrap();
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
            .any(|r| r.enabled.is_err() && r.text == "Multiplayer")
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
    quit_session(&mut app);
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

    // Esc pauses; the pause menu's Quit row is quit-to-menu, not
    // quit-to-exit.
    quit_session(&mut app);
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
    quit_session(&mut app);
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
        // No driver, no progress to count — just the table size.
        assert_eq!(rows[0].text, "Checkpoint (4 races)");
    }
    activate_row(&mut app, "Checkpoint");

    // The event list: one row per event, its options beside it —
    // race0/race1 open, race2 incomplete, race3 gated.
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
    assert!(
        shell(&app)
            .rows
            .iter()
            .all(|r| r.side.as_ref().is_some_and(|o| o.text == menu::OPTIONS)),
        "every event carries its options beside it"
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

/// The checkpoint install plus a two-row Crash Course table — `crash0`
/// a one-leg lesson, `crash1` an exam row — for the F21-B.8 menu entry.
fn crash_install() -> tempfile::TempDir {
    let tmp = install();
    let d = tmp.path();
    let row = "0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n";
    write(
        d,
        "race/testcity/mmcrashdata.csv",
        format!("{MM_HEADER}\nlesson1,{row}midtrm1,{row}"),
    );
    let crashdata =
        "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";
    for stem in ["crash0", "crash1"] {
        write(d, &format!("race/testcity/{stem}.aimap"), "[Opponent]\n0\n");
        for suffix in ["data", "data_p"] {
            write(
                d,
                &format!("race/testcity/{stem}{suffix}.csv"),
                format!("{crashdata}slalom,7,1,12,0.05,0,0,0,0,0,0\n"),
            );
        }
    }
    write(d, "race/testcity/slalom.csv", waypoints_csv());
    tmp
}

/// F21-B.8: the Crash Course table is a real menu entry — lessons are
/// listed by authored name, their options stay closed (a lesson runs as
/// authored), the midterm keeps its authored gate on the lessons, and an
/// open row launches as a lesson (driver installed) instead of the old "not loadable yet" refusal.
#[test]
fn crash_course_rows_launch_as_lessons() {
    let tmp = crash_install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    {
        let rows = &shell(&app).rows;
        let crash = rows
            .iter()
            .find(|r| r.text.starts_with("Crash Course"))
            .unwrap();
        assert!(crash.enabled.is_ok(), "{:?}", crash.enabled);
        assert_eq!(crash.text, "Crash Course (2 lessons)");
    }
    activate_row(&mut app, "Crash Course");
    let listed: Vec<(String, Result<(), String>)> = shell(&app)
        .rows
        .iter()
        .map(|r| (r.text.clone(), r.enabled.clone()))
        .collect();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(listed[0].0, "Lesson 1");
    assert_eq!(listed[1].0, "Midterm 1");
    let options = options_of(&app, "Lesson 1");
    assert_eq!(
        options.enabled.as_ref().unwrap_err(),
        "crash course lessons run as authored"
    );

    // The authored gate stands: the midterm names the lesson it needs
    // and activating it is a status line, never a launch.
    assert!(
        listed[1].1.as_ref().unwrap_err().contains("crash0"),
        "{:?}",
        listed[1].1
    );
    assert!(listed[0].1.is_ok(), "{:?}", listed[0].1);
    activate_row(&mut app, "Midterm 1");
    assert_eq!(phase(&app), SessionPhase::Menu);
    activate_row(&mut app, "Lesson 1");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "the lesson never launched: {:?}",
        phase(&app)
    );
    match app.world().resource::<Session>().config() {
        Some(cfg) => assert_eq!(
            cfg.mode,
            SessionMode::Event(mm2_game::EventRef {
                city: "testcity".into(),
                table: EventTableKind::CrashCourse,
                index: 0,
            })
        ),
        None => panic!("a launched session has a config"),
    }
    assert!(
        app.world()
            .get_resource::<mm2_app::lesson::LessonDriver>()
            .is_some(),
        "a menu-launched Crash Course row runs under a lesson driver"
    );
}

/// A London install with a Crash Course and two cars: `vpt` (the pending
/// selection) and `vpcab` (the school's required vehicle, CC-4).
fn london_cab_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for car in ["vpt", "vpcab"] {
        write(
            d,
            &format!("tune/{car}.info"),
            "Description=Test Car\nColors=Red|Blue\n",
        );
        write(d, &format!("tune/vehicle/{car}.vehcarsim"), vehcarsim());
        write(d, &format!("geometry/{car}.pkg"), car_pkg());
        write(d, &format!("bound/{car}_bound.bnd"), car_bnd());
        write(d, &format!("geometry/{car}_whl0.mtx"), wheel_mtx());
    }
    write(d, "city/london.psdl", city_psdl());
    let row = "0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n";
    write(
        d,
        "race/london/mmcrashdata.csv",
        format!("{MM_HEADER}\nlesson1,{row}"),
    );
    let crashdata =
        "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";
    write(d, "race/london/crash0.aimap", "[Opponent]\n0\n");
    for suffix in ["data", "data_p"] {
        write(
            d,
            &format!("race/london/crash0{suffix}.csv"),
            format!("{crashdata}slalom,7,1,12,0.05,0,0,0,0,0,0\n"),
        );
    }
    write(d, "race/london/slalom.csv", waypoints_csv());
    tmp
}

/// Launch London's Lesson 1 from the menu and report the session's
/// vehicle, the shell's pending selection and the drive-through result.
fn launch_london_lesson(app: &mut App) -> (Option<String>, Option<String>) {
    activate_row(app, "Events");
    activate_row(app, "london");
    activate_row(app, "Crash Course");
    activate_row(app, "Lesson 1");
    assert!(
        run_until(app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "the lesson never launched: {:?}",
        phase(app)
    );
    let cfg = app.world().resource::<Session>().config().cloned().unwrap();
    (cfg.vehicle.id, shell(app).vehicle.id.clone())
}

/// CC-4: an unpassed lesson drives its school's required car whatever the
/// menu has selected — and the pending selection is left alone, so the
/// next cruise still drives the player's pick.
#[test]
fn an_unpassed_lesson_requires_the_schools_vehicle() {
    let tmp = london_cab_install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    let (session_car, pending) = launch_london_lesson(&mut app);
    assert_eq!(session_car.as_deref(), Some("vpcab"));
    assert_eq!(pending.as_deref(), Some("vpt"), "the pick is not rewritten");
    let selected = app.world().resource::<SelectedCar>();
    assert_eq!(selected.def.as_ref().map(|d| d.id.as_str()), Some("vpcab"));
}

/// CC-4's second half: a passed lesson replays in any vehicle, and a
/// lesson's required car never replaces the profile's remembered one.
#[test]
fn a_passed_lesson_replays_in_the_selected_vehicle_and_a_forced_car_is_not_remembered() {
    let tmp = london_cab_install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let id = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap()
        .id;
    let mut loaded = store.load(&id).unwrap().profile;
    loaded.selections.vehicle = Some(mm2_game::VehicleChoice {
        id: "vpt".into(),
        paint: 1,
    });
    store.save(&mut loaded).unwrap();
    let reopen = ProfileStore::open(store_dir.path()).unwrap();

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    let (session_car, _) = launch_london_lesson(&mut app);
    assert_eq!(session_car.as_deref(), Some("vpcab"));
    let after = reopen.load(&id).unwrap().profile;
    let remembered = after.selections.vehicle.expect("the remembered car stays");
    assert_eq!((remembered.id.as_str(), remembered.paint), ("vpt", 1));
    assert!(
        after.selections.last_event.is_some(),
        "the lesson itself is still remembered"
    );

    // Pass the lesson, back to the menu: the pending car now drives.
    let mut passed = reopen.load(&id).unwrap().profile;
    passed
        .event_mut(EventKey {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            stem: "crash0".into(),
        })
        .record_finish(900, Some(1), Difficulty::Amateur);
    reopen.save(&mut passed).unwrap();
    let mut app = menu_app(
        tmp.path(),
        Some(ProfileStore::open(store_dir.path()).unwrap()),
    );
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    let (session_car, _) = launch_london_lesson(&mut app);
    assert_eq!(session_car.as_deref(), Some("vpt"));
}

/// CC-3 end to end on the menu: a profile whose lesson pass was credited
/// (the same `EventRecord` `record_session_results` writes for a passed
/// lesson) finds the midterm open, while a fresh profile's stays gated.
#[test]
fn a_credited_lesson_pass_opens_the_midterm() {
    let tmp = crash_install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    for (name, passed) in [("Fresh", false), ("Alice", true)] {
        let made = store
            .create(name, Difficulty::Amateur, ProfileKind::Standard)
            .unwrap();
        if passed {
            let mut loaded = store.load(&made.id).unwrap().profile;
            loaded
                .event_mut(EventKey {
                    city: "testcity".into(),
                    table: EventTableKind::CrashCourse,
                    stem: "crash0".into(),
                })
                .record_finish(900, Some(1), Difficulty::Amateur);
            store.save(&mut loaded).unwrap();
        }
    }
    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    for (name, open) in [("Fresh", false), ("Alice", true)] {
        activate_row(&mut app, "Driver:");
        activate_row(&mut app, name);
        press(&mut app, KeyCode::Escape);
        activate_row(&mut app, "Events");
        activate_row(&mut app, "testcity");
        activate_row(&mut app, "Crash Course");
        let midterm = shell(&app)
            .rows
            .iter()
            .find(|r| r.text == "Midterm 1")
            .map(|r| r.enabled.clone())
            .expect("the midterm is listed");
        assert_eq!(midterm.is_ok(), open, "{name}: {midterm:?}");
        // Back to the root for the next driver.
        press(&mut app, KeyCode::Escape);
        press(&mut app, KeyCode::Escape);
        press(&mut app, KeyCode::Escape);
    }
}

/// Event rows carry the race names `tune/<city>.cinfo` authors, by
/// table row; a row past the authored list keeps the stem label, and
/// Quick Race shows the same name.
#[test]
fn event_rows_show_authored_race_names() {
    let tmp = install();
    write(
        tmp.path(),
        "tune/testcity.cinfo",
        "LocalizedName=Test City\r\nCheckpointNames=Racing 101|Deck The Hall\r\n",
    );
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let mut loaded = store.load(&alice.id).unwrap().profile;
    loaded.selections.last_event = Some(EventKey {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        stem: "race1".into(),
    });
    store.save(&mut loaded).unwrap();

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    assert_eq!(quick_race(&app).0, "Quick Race: Deck The Hall");

    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        texts,
        [
            "Racing 101",
            "Deck The Hall",
            "Checkpoint #2 (race2)",
            "Checkpoint #3 (race3)"
        ]
    );
}

/// F29 (localization consumer): a mod's `tune/<city>.cinfo` renames the
/// races the menu lists and Quick Race names, the install's own names
/// return when the mod is not mounted, and the replacement is
/// cosmetic-only — it neither moves the gameplay fingerprint nor makes the
/// mod record-ineligible, so a translated install can still join and
/// record.
#[test]
fn a_mods_cinfo_renames_the_races_and_is_cosmetic() {
    let tmp = install();
    write(
        tmp.path(),
        "tune/testcity.cinfo",
        "LocalizedName=Test City\r\nCheckpointNames=Racing 101|Deck The Hall\r\n",
    );
    let mod_dir = tempfile::tempdir().unwrap();
    write(mod_dir.path(), "mod.toml", "[mod]\nid = \"fr\"\n");
    write(
        mod_dir.path(),
        "tune/testcity.cinfo",
        "LocalizedName=Ville d'essai\nCheckpointNames=Course 101|Joyeux Noel\n",
    );
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let mut loaded = store.load(&alice.id).unwrap().profile;
    loaded.selections.last_event = Some(EventKey {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        stem: "race1".into(),
    });
    store.save(&mut loaded).unwrap();

    // The menu's view of one mount set: Quick Race's label and the
    // Checkpoint list.
    let observe = |mods: &[&Path]| {
        let mut vfs = vfs_of(tmp.path());
        for (i, m) in mods.iter().enumerate() {
            vfs.mount_mod(m, 300 + i as i32).unwrap();
        }
        let hash = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
        let cosmetic = mm2_content::fingerprint::mods_cosmetic_only(&vfs);
        let store = ProfileStore::open(store_dir.path()).unwrap();
        let mut app = menu_app_vfs(vfs, MenuData::new(Some(store), false, None));
        app.update();
        activate_row(&mut app, "Driver:");
        activate_row(&mut app, "Alice");
        press(&mut app, KeyCode::Escape);
        let quick = quick_race(&app).0;
        activate_row(&mut app, "Events");
        activate_row(&mut app, "testcity");
        activate_row(&mut app, "Checkpoint");
        let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
        (quick, rows, hash, cosmetic)
    };

    let (quick, rows, base_hash, _) = observe(&[]);
    assert_eq!(quick, "Quick Race: Deck The Hall");
    assert_eq!(rows[..2], ["Racing 101", "Deck The Hall"]);

    let (quick, rows, hash, cosmetic) = observe(&[mod_dir.path()]);
    assert_eq!(quick, "Quick Race: Joyeux Noel");
    assert_eq!(
        rows,
        [
            "Course 101",
            "Joyeux Noel",
            "Checkpoint #2 (race2)",
            "Checkpoint #3 (race3)"
        ]
    );
    assert_eq!(hash, base_hash, "display names are not gameplay content");
    assert!(cosmetic, "a translation mod keeps records and joining");

    // Unmounted again: the install's own names.
    assert_eq!(observe(&[]).0, "Quick Race: Deck The Hall");
}

/// Won races carry a badge in the event list: a beaten record marks
/// its row with the difficulties it was beaten at, while a finish that
/// missed the place criterion (or no record at all) leaves it bare.
#[test]
fn event_rows_mark_won_races() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let key = |stem: &str| EventKey {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        stem: stem.into(),
    };
    seed_record(&store, &alice.id, key("race0"), 120 * 60, Some(1));
    seed_record(&store, &alice.id, key("race1"), 120 * 60, Some(5));

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    // The race-type selector counts won events against the table.
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(texts[0], "Checkpoint (1/4 won)", "{texts:?}");
    assert_eq!(texts[1], "Blitz (no races)", "{texts:?}");
    activate_row(&mut app, "Checkpoint");
    let won = |stem: &str| {
        shell(&app)
            .rows
            .iter()
            .find(|r| r.text.contains(stem))
            .unwrap()
            .won
    };
    assert_eq!(
        won("race0"),
        Some(menu::Won {
            amateur: true,
            professional: false
        })
    );
    assert_eq!(won("race0").unwrap().label(), "WON");
    assert_eq!(won("race1"), None, "a 5th place is a finish, not a win");
    assert_eq!(won("race3"), None);
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

/// A locked car or paint names what earns it: milestone rewards count
/// the driver's won races in the family against the target, indexed
/// rewards name their event.
#[test]
fn locked_vehicles_name_their_unlock_requirement() {
    let tmp = install();
    let d = tmp.path();
    // A second complete car, reward-gated behind half the checkpoints.
    write(d, "tune/vpu.info", "Description=Locked Car\nColors=Green\n");
    write(d, "tune/vehicle/vpu.vehcarsim", vehcarsim());
    write(d, "geometry/vpu.pkg", car_pkg());
    write(d, "bound/vpu_bound.bnd", car_bnd());
    write(d, "geometry/vpu_whl0.mtx", wheel_mtx());
    write(
        d,
        "race/testcity/testcity_rewards.csv",
        format!(
            "{REWARDS}race,0,vpt,1,Beaten event zero paint,\n\
             race,half,vpu,0,Half the checkpoints,\n"
        ),
    );
    write(
        d,
        "tune/testcity.cinfo",
        "LocalizedName=Test City\nCheckpointNames=Racing 101\n",
    );
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    seed_record(
        &store,
        &alice.id,
        EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: "race1".into(),
        },
        120 * 60,
        Some(2),
    );

    let mut app = menu_app(d, Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Vehicle:");
    let locked = shell(&app)
        .rows
        .iter()
        .find(|r| r.text.contains("Locked Car"))
        .unwrap()
        .enabled
        .clone();
    assert_eq!(
        locked,
        Err("locked - win 2 of the 4 Checkpoint races in Test City (1/2 won)".to_string())
    );

    activate_row(&mut app, "Test Car");
    let blue = shell(&app)
        .rows
        .iter()
        .find(|r| r.text.contains("Blue"))
        .unwrap()
        .enabled
        .clone();
    assert_eq!(
        blue,
        Err("locked - pass Racing 101 in Test City".to_string())
    );
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

    // A fresh driver is named on the entry screen — creating binds it
    // and pops back to the profile list.
    activate_row(&mut app, "New driver");
    assert!(
        matches!(shell(&app).screen, menu::Screen::NewProfile { .. }),
        "New driver opens the name-entry screen"
    );
    type_text(&mut app, "Dave");
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(shell(&app).screen, menu::Screen::Profiles),
        "a successful create returns to the profile list"
    );
    app.update();
    let created = app
        .world()
        .get_resource::<ActiveProfile>()
        .expect("create binds the new profile")
        .profile
        .id
        .clone();
    assert_eq!(
        store.load(&created).unwrap().profile.name,
        "Dave",
        "the typed name is the profile's name"
    );

    // Carol (not bound, not last) still deletes.
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

/// The name-entry screen owns the keyboard: typed characters append,
/// Backspace erases, Enter creates and binds with the typed name, and
/// the field's screen is a text field — not a row list.
#[test]
fn new_driver_entry_types_edits_and_binds() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut app = menu_app(tmp.path(), Some(store.clone()));
    app.update();

    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "New driver");
    assert_eq!(name_buffer(&app), "");
    assert!(
        shell(&app).rows.is_empty(),
        "the entry screen is a text field, not a row list"
    );

    // Typing goes through the real KeyboardInput message path;
    // Backspace edits.
    type_text(&mut app, "Ada Lovelacex");
    assert_eq!(name_buffer(&app), "Ada Lovelacex");
    erase(&mut app);
    assert_eq!(name_buffer(&app), "Ada Lovelace");
    assert!(
        menu_texts(&mut app)
            .iter()
            .any(|t| t == "  Name: Ada Lovelace_"),
        "the entry field draws the live buffer"
    );

    press(&mut app, KeyCode::Enter);
    assert!(matches!(shell(&app).screen, menu::Screen::Profiles));
    let bound = app
        .world()
        .get_resource::<ActiveProfile>()
        .expect("create binds the typed profile");
    assert_eq!(bound.profile.name, "Ada Lovelace");
    assert!(
        store
            .list()
            .unwrap()
            .iter()
            .any(|p| p.meta.as_ref().is_some_and(|m| m.name == "Ada Lovelace")),
        "the typed name is persisted in the store"
    );
}

/// Enter on an empty field refuses with a visible reason and stays;
/// Esc cancels without creating anything, and a reopened field starts
/// empty — the cancelled text doesn't linger.
#[test]
fn new_driver_entry_refuses_empty_and_cancels() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut app = menu_app(tmp.path(), Some(store.clone()));
    app.update();

    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "New driver");

    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(shell(&app).screen, menu::Screen::NewProfile { .. }),
        "an empty name keeps the entry screen open"
    );
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("type a name first"),
        "the refusal names the reason"
    );
    assert!(store.list().unwrap().is_empty(), "no profile was created");

    // A whitespace-only name trims to the same refusal.
    type_text(&mut app, "   ");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        shell(&app).screen,
        menu::Screen::NewProfile { .. }
    ));
    assert!(store.list().unwrap().is_empty());

    // Esc cancels: back to the list, nothing created, and reopening
    // starts from an empty buffer.
    press(&mut app, KeyCode::Escape);
    assert!(matches!(shell(&app).screen, menu::Screen::Profiles));
    assert!(store.list().unwrap().is_empty());
    activate_row(&mut app, "New driver");
    assert_eq!(name_buffer(&app), "");
}

/// Characters the embedded font cannot draw are refused with a status
/// note instead of storing a name that renders as tofu; the field is
/// bounded at the store's name limit; and nav keys on the entry
/// screen move nothing — Space is a character, not Activate.
#[test]
fn new_driver_entry_bounds_and_filters_input() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut app = menu_app(tmp.path(), Some(store.clone()));
    app.update();

    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "New driver");

    // Non-ASCII input is refused with a note; ASCII appends and clears
    // the note again.
    type_text(&mut app, "é");
    assert_eq!(name_buffer(&app), "");
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("names use ASCII characters only")
    );
    type_text(&mut app, "ab");
    assert_eq!(name_buffer(&app), "ab");
    assert!(shell(&app).status.is_none());

    // Arrow keys produce no text and move no focus — the buffer is
    // untouched and no row list exists to focus.
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::ArrowUp);
    assert_eq!(name_buffer(&app), "ab");

    // Space is a character on the entry screen, not Activate.
    type_text(&mut app, " ");
    assert_eq!(name_buffer(&app), "ab ");
    assert!(matches!(
        shell(&app).screen,
        menu::Screen::NewProfile { .. }
    ));

    // The buffer stops at the store's name bound (MAX_NAME_CHARS = 32)
    // with the limit named on the status line.
    type_text(&mut app, &"x".repeat(40));
    assert_eq!(name_buffer(&app).chars().count(), 32);
    assert!(
        shell(&app)
            .status
            .as_deref()
            .is_some_and(|s| s.contains("32")),
        "the cap refusal names the bound"
    );

    // The bounded, trimmed name still creates.
    erase(&mut app);
    erase(&mut app);
    erase(&mut app);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(shell(&app).screen, menu::Screen::Profiles));
    assert_eq!(
        store.list().unwrap().len(),
        1,
        "the bounded name created one profile"
    );
}

/// The Quick Race row's enabled state under `shell` — the focused
/// assertions for both Quick Race tests.
fn quick_race(app: &App) -> (String, Result<(), String>) {
    shell(app)
        .rows
        .iter()
        .find(|r| r.text.starts_with("Quick Race"))
        .map(|r| (r.text.clone(), r.enabled.clone()))
        .unwrap_or_else(|| {
            let rows: Vec<String> = shell(app).rows.iter().map(|r| r.text.clone()).collect();
            panic!("no Quick Race row — rows: {rows:?}")
        })
}

/// F17-A.2 / DRV-8: a bound profile's `last_event` relaunches straight
/// from the root row. The event is resolved stem-keyed through the live
/// catalog, the profile file itself records the stem on session start,
/// and activating the row lands in the same `EventRef` the event list
/// would have launched.
#[test]
fn quick_race_replays_the_last_event() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();

    let mut app = menu_app(tmp.path(), Some(store.clone()));
    app.update();

    // No driver bound: the row exists but explains itself.
    let (_, enabled) = quick_race(&app);
    assert!(
        enabled.unwrap_err().contains("no driver profile"),
        "profile-less runs have no last_event"
    );

    // Bind Alice — she has never played an event.
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    let (_, enabled) = quick_race(&app);
    assert!(
        enabled.unwrap_err().contains("no event played yet"),
        "a fresh profile has nothing to replay"
    );

    // Play race0 through the real Events path — `note_session_start`
    // records the stem-keyed last_event at session start.
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    activate_row(&mut app, "race0");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "race0 never launched: {:?}",
        phase(&app)
    );
    let saved = store.load(&alice.id).unwrap().profile;
    assert_eq!(
        saved.selections.last_event,
        Some(EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: "race0".into(),
        }),
        "the session start must persist the stem-keyed event"
    );

    // Quit-to-menu — the root row now names the last event.
    quit_session(&mut app);
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu));
    app.update();
    let (text, enabled) = quick_race(&app);
    assert_eq!(text, "Quick Race: Checkpoint #0 (race0)");
    assert!(
        enabled.is_ok(),
        "the replayed event must be open: {enabled:?}"
    );

    // Activating it launches the same authored event — no walk through
    // the event tree, no fake quick-race mode.
    activate_row(&mut app, "Quick Race");
    assert!(run_until(&mut app, 12, |a| matches!(
        phase(a),
        SessionPhase::Countdown | SessionPhase::Playing
    )));
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

/// A `last_event` that no longer resolves — gated, incomplete, or
/// absent from the catalog — disables Quick Race with the reason; the
/// row never launches a different event at the stale index.
#[test]
fn quick_race_reports_an_unresolvable_last_event() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let cases = [
        // race3 is gated by CHK-3 on the unbeaten first set.
        ("Gated", "race3", "beat"),
        // race2 has no waypoint record — the catalog knows it but
        // cannot produce a race.
        ("Broken", "race2", "incomplete"),
        // A stem no mounted table row claims (deleted mod, older save).
        ("Gone", "race99", "not in the testcity catalog"),
    ];
    for (name, stem, _) in &cases {
        let profile = store
            .create(*name, Difficulty::Amateur, ProfileKind::Standard)
            .unwrap();
        let mut loaded = store.load(&profile.id).unwrap().profile;
        loaded.selections.last_event = Some(EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: stem.to_string(),
        });
        store.save(&mut loaded).unwrap();
    }

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();

    for (name, stem, reason) in &cases {
        activate_row(&mut app, "Driver:");
        activate_row(&mut app, name);
        press(&mut app, KeyCode::Escape);
        let (text, enabled) = quick_race(&app);
        assert!(
            text.contains(stem),
            "{name}: row should name {stem}: {text}"
        );
        let err = enabled.unwrap_err();
        assert!(
            err.contains(reason),
            "{name}: expected {reason:?} in {err:?}"
        );
        // The disabled row never launches.
        activate_row(&mut app, "Quick Race");
        assert_eq!(phase(&app), SessionPhase::Menu, "{name} must not launch");
        assert!(shell(&app).status.is_some());
    }
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

/// Regression guard for the restart transit: a pause-menu Restart
/// transits `Menu` for one update while `drive_session` re-`begin`s —
/// `menu_watch` must not reopen the shell mid-transit. A reopened
/// shell would draw over the new session (menu root + menu camera
/// mid-game) and steal its keys (menu `Back` reaching `Exit` while
/// `Playing`). Asserted per-update so an ordering shift can't hide it.
#[test]
fn restart_in_menu_mode_leaves_the_shell_closed() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));

    // Esc → pause → Restart row (the second row).
    press(&mut app, KeyCode::Escape);
    assert_eq!(phase(&app), SessionPhase::Paused);
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::Enter);

    let mut replayed = false;
    for _ in 0..12 {
        app.update();
        assert!(
            !shell(&app).active,
            "the shell must never reopen mid-restart (phase {:?})",
            phase(&app)
        );
        // A loading splash may render, but the interactive shell stays closed.
        let splash = menu_roots(&mut app);
        assert!(splash <= 1, "duplicate restart UI");
        assert_eq!(menu_cameras(&mut app), splash, "unexpected restart camera");
        if splash == 1 {
            assert!(
                menu_texts(&mut app)
                    .iter()
                    .any(|t| t == "LOADING THE STREETS")
            );
        }
        let world = app.world_mut();
        assert_eq!(
            world.query::<&menu::MenuRow>().iter(world).count(),
            0,
            "interactive rows appeared during restart"
        );
        if phase(&app) == SessionPhase::Playing {
            replayed = true;
            break;
        }
    }
    assert!(replayed, "the restart never reached Playing");
    assert_eq!(players(&mut app), 1);

    // The shell stays closed in the new session: Esc pauses, it does
    // not run a menu Back → Exit.
    press(&mut app, KeyCode::Escape);
    assert_eq!(
        phase(&app),
        SessionPhase::Paused,
        "Esc must pause, not exit"
    );
    assert!(
        app.world().resource::<Messages<AppExit>>().is_empty(),
        "no AppExit from a restart transit"
    );
}

/// F17-AC04's return leg: a launch whose world fails to load lands
/// back on the menu with the reason on the status line — the user is
/// told *why*, not silently returned.
#[test]
fn a_failed_launch_returns_to_the_menu_with_the_reason() {
    let tmp = install();
    // Resolves (so the city row is enabled) but cannot parse.
    write(tmp.path(), "city/broken.psdl", b"junk");
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "broken");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Failed(_)
        )),
        "the broken city never failed: {:?}",
        phase(&app)
    );

    press(&mut app, KeyCode::Escape);
    assert!(
        run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu),
        "quit from Failed never reached the menu"
    );
    app.update();
    assert!(shell(&app).active, "the menu reopens after a failure");
    let status = shell(&app).status.as_deref().unwrap_or("");
    assert!(
        status.contains("load failed"),
        "the failure reason lands on the status line: {status:?}"
    );
    assert_eq!(menu_roots(&mut app), 1);
}

/// F17-A.4: the mouse path. Hover focuses the row under the cursor,
/// left-click activates it through `MenuCommand::Activate`, right-click
/// backs out — and a resting cursor is an edge, not a pin, so it never
/// fights keyboard/gamepad focus.
#[test]
fn the_mouse_focuses_rows_and_clicks_drive_the_same_commands() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    spawn_window(&mut app);
    lay_out_rows(&mut app);

    // The visible page preserves model indices for mouse hit testing.
    let world = app.world_mut();
    let count = world.query::<&menu::MenuRow>().iter(world).count();
    assert_eq!(count, shell(&app).rows.len().min(9));

    // Empty space focuses nothing.
    cursor_to(&mut app, 200.0, 10.0);
    app.update();
    assert_eq!(shell(&app).focus, 0);

    // Hover moves focus to the row under the cursor.
    cursor_to(&mut app, 200.0, row_y(2));
    app.update();
    assert_eq!(shell(&app).focus, 2);

    // A resting cursor is an edge, not a held state — keyboard nav
    // still moves focus afterwards and the mouse does not re-assert.
    // (The focus change respawned the row entities, so re-lay them
    // out before the next update hit-tests.)
    press(&mut app, KeyCode::ArrowUp);
    assert_eq!(shell(&app).focus, 1);
    lay_out_rows(&mut app);
    app.update();
    assert_eq!(shell(&app).focus, 1);

    // Moving over another row focuses it; left-click activates it
    // through `Activate` — Cruise pushes the city picker.
    cursor_to(&mut app, 200.0, row_y(0));
    click(&mut app, MouseButton::Left);
    assert!(matches!(shell(&app).screen, menu::Screen::CruiseCity));

    // Right-click backs out from anywhere — the mouse's Esc.
    click(&mut app, MouseButton::Right);
    assert!(matches!(shell(&app).screen, menu::Screen::Root));
}

/// Options sit beside their row instead of between rows: Up/Down walk
/// only the launchable entries, Right steps onto the focused row's
/// options and Left back, the column survives vertical moves and a
/// round trip through the options screen, and Enter on the row itself
/// launches.
#[test]
fn options_sit_to_the_right_of_their_row() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");

    // Down walks events, not options.
    press(&mut app, KeyCode::ArrowDown);
    assert_eq!(shell(&app).focus, 1);
    assert!(shell(&app).rows[1].text.contains("race1"));

    // Right steps onto the options; the focused entry is now the
    // side entry, Left steps back.
    press(&mut app, KeyCode::ArrowRight);
    assert!(shell(&app).side);
    assert_eq!(
        shell(&app).focused_row().map(|r| r.text.as_str()),
        Some(menu::OPTIONS)
    );
    press(&mut app, KeyCode::ArrowLeft);
    assert!(!shell(&app).side);

    // The column survives a vertical move.
    press(&mut app, KeyCode::ArrowRight);
    press(&mut app, KeyCode::ArrowUp);
    assert_eq!(shell(&app).focus, 0);
    assert!(shell(&app).side, "Up keeps the options column");

    // Cruise options open with Right + Enter, and Back returns to the
    // options column the user left from.
    for _ in 0..3 {
        press(&mut app, KeyCode::Escape);
    }
    activate_row(&mut app, "Cruise");
    open_options(&mut app, "testcity");
    assert!(matches!(shell(&app).screen, menu::Screen::Customize { .. }));
    press(&mut app, KeyCode::Escape);
    assert!(matches!(shell(&app).screen, menu::Screen::CruiseCity));
    assert!(shell(&app).side);

    // Back on the row itself, Enter launches.
    press(&mut app, KeyCode::ArrowLeft);
    press(&mut app, KeyCode::Enter);
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
}

/// The mouse reaches a side entry directly: hovering the options cell
/// focuses it, clicking opens it, and hovering the row proper moves
/// focus back off the cell.
#[test]
fn the_mouse_hovers_and_clicks_options_cells() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    spawn_window(&mut app);
    activate_row(&mut app, "Cruise");
    lay_out_rows(&mut app);
    lay_out_sides(&mut app);

    cursor_to(&mut app, 520.0, row_y(0));
    app.update();
    assert!(shell(&app).side, "hovering the cell focuses the options");

    // The focus change respawned the row entities — re-lay them out.
    lay_out_rows(&mut app);
    lay_out_sides(&mut app);
    cursor_to(&mut app, 200.0, row_y(0));
    app.update();
    assert!(!shell(&app).side, "hovering the row focuses the row");

    lay_out_rows(&mut app);
    lay_out_sides(&mut app);
    cursor_to(&mut app, 520.0, row_y(0));
    click(&mut app, MouseButton::Left);
    assert!(matches!(shell(&app).screen, menu::Screen::Customize { .. }));
}

/// Place every `MenuSide` cell as `lay_out_rows` places rows: an 80x22
/// rect centred at `(520, row_y(index))`, clear of the row rects.
fn lay_out_sides(app: &mut App) {
    let world = app.world_mut();
    let mut q = world.query::<(Entity, &menu::MenuSide)>();
    let sides: Vec<(Entity, usize)> = q.iter(world).map(|(e, r)| (e, r.index)).collect();
    for (entity, index) in sides {
        world.entity_mut(entity).insert((
            ComputedNode {
                size: Vec2::new(80.0, 22.0),
                ..default()
            },
            UiGlobalTransform::from_xy(520.0, row_y(index)),
        ));
    }
}

/// F17-A.4 negative leg: clicking a disabled row focuses it and
/// surfaces its reason — the same `Activate` path a keyboard press
/// takes — instead of navigating.
#[test]
fn a_click_on_a_disabled_row_shows_its_reason() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    spawn_window(&mut app);
    lay_out_rows(&mut app);

    let stats = shell(&app)
        .rows
        .iter()
        .position(|r| r.text == "Driver's Stats")
        .expect("the root screen lists Driver's Stats");
    cursor_to(&mut app, 200.0, row_y(stats));
    click(&mut app, MouseButton::Left);
    assert!(matches!(shell(&app).screen, menu::Screen::Root));
    assert_eq!(shell(&app).focus, stats);
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("not implemented yet (menu audit: docs/research/menu.md)"),
    );
}

/// Regression for the F17-A.4 review quirk: a right-click over a row
/// backs out — and must not also queue the hover `FocusAt`, which
/// would apply *after* the pop and clobber the parent's restored
/// focus with a stale child index.
#[test]
fn a_right_click_backs_out_without_clobbering_the_restored_focus() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    spawn_window(&mut app);

    // Descend into the city picker: Root's focus (the Events row)
    // is saved on the back stack.
    let events_index = shell(&app)
        .rows
        .iter()
        .position(|r| r.text == "Events")
        .expect("the root lists Events");
    activate_row(&mut app, "Events");
    assert!(matches!(shell(&app).screen, menu::Screen::EventCity));
    lay_out_rows(&mut app);
    cursor_to(&mut app, 200.0, row_y(1));
    click(&mut app, MouseButton::Right);
    assert!(matches!(shell(&app).screen, menu::Screen::Root));
    assert_eq!(
        shell(&app).focus,
        events_index,
        "Back restores the saved focus — the click must not hover-focus a root row"
    );
}

/// DRV-5's first leg: the Records screen lists the bound driver's
/// persisted results — best time, best place and finish count — and
/// an enabled record row re-launches the same event through
/// `Session::begin`.
#[test]
fn the_records_screen_shows_persisted_results_and_relaunches() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // Two finishes on race0 — the best time keeps the faster run and
    // the Amateur top-3 win sets the [A] beaten mark.
    let key0 = || EventKey {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        stem: "race0".into(),
    };
    seed_record(&store, &alice.id, key0(), 120 * 90 + 60, Some(1));
    seed_record(&store, &alice.id, key0(), 120 * 95, Some(2));

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();

    // Unbound: the root row exists but explains itself.
    let records = shell(&app)
        .rows
        .iter()
        .find(|r| r.text == "Race Records")
        .expect("the root lists Race Records");
    assert!(
        records
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("no driver profile"),
        "records are per-driver: {:?}",
        records.enabled
    );

    // Bind Alice and open the screen.
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Race Records");
    assert!(matches!(shell(&app).screen, menu::Screen::Records { .. }));

    let rows = &shell(&app).rows;
    assert_eq!(rows[0].text, "City: all");
    assert_eq!(rows[1].text, "Race type: all");
    let race0 = &rows[2];
    assert!(race0.text.contains("race0"), "{:?}", race0.text);
    assert!(
        race0.text.contains("1:30.5"),
        "the best (lowest) time shows: {:?}",
        race0.text
    );
    assert!(race0.text.contains("place 1"), "{:?}", race0.text);
    assert!(
        race0.text.contains("x2"),
        "the finish count shows: {:?}",
        race0.text
    );
    assert!(
        race0.text.contains("[A]"),
        "the Amateur beaten mark shows: {:?}",
        race0.text
    );
    assert!(race0.enabled.is_ok(), "{:?}", race0.enabled);

    // Activating re-runs the same authored event — no fake
    // records-screen launch mode.
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

/// A record whose event no longer resolves — gated, incomplete or
/// absent from the catalog — still lists with its stored numbers and
/// the reason it cannot re-launch; records order deterministically
/// (city, authored table order, stem) regardless of finish order.
#[test]
fn unresolvable_records_stay_listed_with_their_reasons() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let bob = store
        .create("Bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let key = |stem: &str, table: EventTableKind| EventKey {
        city: "testcity".into(),
        table,
        stem: stem.into(),
    };
    // Seeded out of display order on purpose: the screen sorts.
    seed_record(
        &store,
        &bob.id,
        key("race99", EventTableKind::Checkpoint),
        120 * 80,
        Some(2),
    );
    seed_record(
        &store,
        &bob.id,
        key("race3", EventTableKind::Checkpoint),
        120 * 60,
        Some(4),
    );
    seed_record(
        &store,
        &bob.id,
        key("race2", EventTableKind::Checkpoint),
        120 * 70,
        Some(3),
    );
    seed_record(
        &store,
        &bob.id,
        key("crash0", EventTableKind::CrashCourse),
        120 * 50,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Bob");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Race Records");

    let rows = &shell(&app).rows;
    let records: Vec<&menu::Row> = rows.iter().skip(2).collect();
    assert_eq!(records.len(), 4);
    // Checkpoint stems sort lexically; the Crash Course record sorts
    // after the whole Checkpoint set by authored table order.
    let order: Vec<&str> = records.iter().map(|r| r.text.as_str()).collect();
    assert!(
        order[0].contains("race2")
            && order[1].contains("race3")
            && order[2].contains("race99")
            && order[3].contains("crash0"),
        "deterministic order: {order:?}"
    );
    let by_stem = |stem: &str| records.iter().find(|r| r.text.contains(stem)).unwrap();
    // race3 is gated by CHK-3 on the unbeaten first set — its record
    // exists but the row names the gate.
    assert!(
        by_stem("race3")
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("beat"),
        "{:?}",
        by_stem("race3").enabled
    );
    assert!(
        by_stem("race2")
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("incomplete"),
        "{:?}",
        by_stem("race2").enabled
    );
    assert!(
        by_stem("race99")
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("not in the testcity catalog"),
        "{:?}",
        by_stem("race99").enabled
    );
    assert!(
        by_stem("crash0")
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("not in the testcity catalog"),
        "{:?}",
        by_stem("crash0").enabled
    );
    // Disabled rows still show the stored numbers.
    assert!(
        by_stem("race3").text.contains("x1"),
        "the finish count shows: {}",
        by_stem("race3").text
    );
    // And activating one never launches.
    activate_row(&mut app, "race3");
    assert_eq!(phase(&app), SessionPhase::Menu);
    assert!(shell(&app).status.is_some());
}

/// The filter rows cycle through `all` plus the values actually
/// present in the records — narrowing and widening the list in
/// place, no navigation. Left steps back, Right and Enter step
/// forward.
#[test]
fn records_filters_narrow_and_widen_the_list() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let carol = store
        .create("Carol", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let key = |stem: &str, table: EventTableKind| EventKey {
        city: "testcity".into(),
        table,
        stem: stem.into(),
    };
    seed_record(
        &store,
        &carol.id,
        key("race0", EventTableKind::Checkpoint),
        120 * 60,
        Some(1),
    );
    // A Blitz record no mounted table row claims — it still filters
    // and lists (disabled), like the Quick Race stale-event leg.
    seed_record(
        &store,
        &carol.id,
        key("blitz0", EventTableKind::Blitz),
        120 * 45,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Carol");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Race Records");
    assert_eq!(shell(&app).rows.len(), 4);

    // Race type: all → Checkpoint → Blitz → all, narrowing the list.
    focus_row(&mut app, "Race type:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[1].text, "Race type: Checkpoint");
    let listed: Vec<&str> = shell(&app)
        .rows
        .iter()
        .skip(2)
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].contains("race0"), "{listed:?}");

    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[1].text, "Race type: Blitz");
    let listed: Vec<&str> = shell(&app)
        .rows
        .iter()
        .skip(2)
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].contains("blitz0"), "{listed:?}");

    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[1].text, "Race type: all");
    assert_eq!(shell(&app).rows.len(), 4);

    // Left walks back — straight to the last value from `all`.
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[1].text, "Race type: Blitz");

    // The city filter cycles the same way (one city in the records).
    focus_row(&mut app, "City:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[0].text, "City: testcity");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[0].text, "City: all");
    // Enter on a filter row cycles forward, same as Right.
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).rows[0].text, "City: testcity");
}

/// Records are per-driver: a fresh profile opens the screen to the
/// honest empty state, and Alice's records never leak into it.
#[test]
fn a_fresh_profile_opens_records_to_the_empty_state() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store
        .create("Bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    seed_record(
        &store,
        &alice.id,
        EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: "race0".into(),
        },
        120 * 60,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Bob");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Race Records");
    let rows = &shell(&app).rows;
    assert_eq!(rows.len(), 1, "no filter rows without records");
    assert!(
        rows[0]
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("no recorded results"),
        "{:?}",
        rows[0].enabled
    );
    // Bob has no records and sees none of Alice's.
    assert!(
        !rows.iter().any(|r| r.text.contains("race0")),
        "another driver's records must not show"
    );
}

/// RACE-3's gate (F17-A.6): an event's options row opens only once its
/// own record is beaten — profile-less play, an unbeaten event and a
/// CHK-3-locked event all keep the row visible with its reason.
#[test]
fn event_options_unlock_only_after_the_race_is_beaten() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    store
        .create("Bob", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();

    // Unbound: the options row names the per-driver requirement.
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    assert_eq!(shell(&app).rows.len(), 4, "one row per event");
    let race0_options = options_of(&app, "race0");
    assert!(
        race0_options
            .enabled
            .as_ref()
            .unwrap_err()
            .contains("no driver profile"),
        "{:?}",
        race0_options.enabled
    );
    // The CHK-3-gated race3's options share its launch block reason,
    // and the incomplete race2's share its missing-records reason.
    let race3_options = &options_of(&app, "race3").enabled;
    assert!(
        race3_options.as_ref().unwrap_err().contains("beat race0"),
        "{race3_options:?}"
    );
    let race2_options = &options_of(&app, "race2").enabled;
    assert!(
        race2_options.as_ref().unwrap_err().contains("incomplete"),
        "{race2_options:?}"
    );

    // Bind Bob — a driver with nothing beaten gets the unlock reason.
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Bob");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    let race0_options = &options_of(&app, "race0").enabled;
    assert!(
        race0_options
            .as_ref()
            .unwrap_err()
            .contains("beat this race"),
        "{race0_options:?}"
    );
}

/// The customized event launch (F17-A.6): with the race beaten the
/// options row opens a screen seeded from the authored params, picks
/// adjust in place, and `Start race` carries them on the launched
/// `SessionConfig::customization` — through to the bound environment.
#[test]
fn customized_event_launch_carries_the_picks() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // race0's authored params: TimeofDay 0 / Weather 0 / Ambient 0.1.
    seed_record(
        &store,
        &alice.id,
        EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: "race0".into(),
        },
        120 * 60,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");

    // race0's options entry is enabled and opens the seeded screen.
    assert!(options_of(&app, "race0").enabled.is_ok());
    open_options(&mut app, "race0");
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(texts[0], "Weather: clear");
    assert_eq!(texts[1], "Time of day: morning");
    assert_eq!(texts[2], "Traffic density: 10%");
    assert_eq!(texts[3], "Start race");

    // Cycle time-of-day one step and traffic one step.
    focus_row(&mut app, "Time of day:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[1].text, "Time of day: noon");
    focus_row(&mut app, "Traffic density:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[2].text, "Traffic density: 25%");

    activate_row(&mut app, "Start race");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "the customized event never launched: {:?}",
        phase(&app)
    );
    let config = app
        .world()
        .resource::<Session>()
        .config()
        .expect("a launched session has a config")
        .clone();
    assert_eq!(
        config.mode,
        SessionMode::Event(mm2_game::EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        })
    );
    let picks = config
        .customization
        .expect("changed picks ride the session config");
    assert_eq!(picks.conditions.time_of_day.get(), 1);
    assert_eq!(picks.conditions.weather.get(), 0);
    assert_eq!(picks.densities.traffic, 0.25);
    // The bound environment is the picked preset's slot, not the
    // authored one (no .ltNN ships in this install → the fallback rig,
    // reported as such).
    let report = app
        .world()
        .resource::<mm2_app::environment::EnvironmentReport>();
    assert_eq!(report.slot, 4, "picked tod 1 ×4 + weather 0");
    assert_eq!(
        report.source,
        mm2_app::environment::ConditionsSource::Customized
    );
    // A customized run is not a default-conditions run (DRV-6).
    assert_eq!(
        mm2_game::record_eligibility(&config),
        Err(mm2_game::Ineligible::Customized)
    );
}

/// A visit that changes nothing launches the default run (DRV-6
/// preserved): `customization` is only set when the picks differ from
/// the session's authored/default seed.
#[test]
fn unchanged_options_launch_a_default_run() {
    let tmp = install();
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    seed_record(
        &store,
        &alice.id,
        EventKey {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            stem: "race0".into(),
        },
        120 * 60,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    open_options(&mut app, "race0");
    activate_row(&mut app, "Start race");
    assert!(run_until(&mut app, 12, |a| matches!(
        phase(a),
        SessionPhase::Countdown | SessionPhase::Playing
    )));
    let config = app
        .world()
        .resource::<Session>()
        .config()
        .expect("a launched session has a config");
    assert!(config.customization.is_none());
    assert_eq!(mm2_game::record_eligibility(config), Ok(()));
}

/// F29 req 5 through the menu: `--mods` reaches the launched session as
/// `mods_active`, and only a mod set the startup classification found
/// cosmetic-only keeps the run record-eligible.
#[test]
fn a_launched_session_carries_the_mod_classification() {
    for (cosmetic_only, expected) in [
        (true, Ok(())),
        (false, Err(mm2_game::Ineligible::ModContent)),
    ] {
        let tmp = install();
        let store_dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(store_dir.path()).unwrap();
        let alice = store
            .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
            .unwrap();
        seed_record(
            &store,
            &alice.id,
            EventKey {
                city: "testcity".into(),
                table: EventTableKind::Checkpoint,
                stem: "race0".into(),
            },
            120 * 60,
            Some(1),
        );
        let data = MenuData::new(Some(store), true, None).with_mods_cosmetic_only(cosmetic_only);
        let mut app = menu_app_with(tmp.path(), data);
        app.update();
        activate_row(&mut app, "Driver:");
        activate_row(&mut app, "Alice");
        press(&mut app, KeyCode::Escape);
        activate_row(&mut app, "Events");
        activate_row(&mut app, "testcity");
        activate_row(&mut app, "Checkpoint");
        open_options(&mut app, "race0");
        activate_row(&mut app, "Start race");
        assert!(run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )));
        let config = app
            .world()
            .resource::<Session>()
            .config()
            .expect("a launched session has a config");
        assert!(config.mods_active);
        assert_eq!(config.mods_cosmetic_only, cosmetic_only);
        assert_eq!(mm2_game::record_eligibility(config), expected);
    }
}

/// RACE-4: cruise condition options are always open — no beaten
/// record, no profile needed — and a changed pick launches a
/// customized cruise.
#[test]
fn cruise_options_launch_a_customized_session() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    activate_row(&mut app, "Cruise");
    open_options(&mut app, "testcity");
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(texts[0], "Weather: clear");
    assert_eq!(texts[3], "Start cruise");

    focus_row(&mut app, "Weather:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[0].text, "Weather: cloudy");
    activate_row(&mut app, "Start cruise");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
    let config = app
        .world()
        .resource::<Session>()
        .config()
        .expect("a launched session has a config");
    assert_eq!(config.mode, SessionMode::Cruise);
    let picks = config
        .customization
        .expect("changed picks ride the session config");
    assert_eq!(picks.conditions.weather.get(), 1);
    assert_eq!(picks.conditions.time_of_day.get(), 0);
}

/// Bind Alice with `circuit0` beaten and navigate to its event list —
/// the shared preamble every race-shape test below drives. The store
/// dir returns with the app so the bound `ProfileStore`'s files stay
/// on disk.
fn beaten_circuit_options(tmp: &tempfile::TempDir) -> (App, tempfile::TempDir) {
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let alice = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // `Opponents`/`NumLaps` read the difficulty's authored block —
    // Amateur here: 2 opponents, 2 laps.
    seed_record(
        &store,
        &alice.id,
        EventKey {
            city: "testcity".into(),
            table: EventTableKind::Circuit,
            stem: "circuit0".into(),
        },
        120 * 60,
        Some(1),
    );

    let mut app = menu_app(tmp.path(), Some(store));
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Alice");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Circuit");
    (app, store_dir)
}

/// RACE-3's Circuit parenthetical (F17-A.7): a beaten Circuit event's
/// options screen carries the Laps and Opponents rows every other
/// table's screen lacks — both seeded from the authored
/// `NumLaps`/`Opponents` block.
#[test]
fn circuit_options_carry_authored_laps_and_opponents() {
    let tmp = circuit_install();
    let (mut app, _store_dir) = beaten_circuit_options(&tmp);

    open_options(&mut app, "circuit0");
    let texts: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        texts,
        vec![
            "Weather: clear",
            "Time of day: morning",
            "Traffic density: 10%",
            "Laps: 2",
            "Opponents: 2",
            "Start race",
        ]
    );
}

/// The race-shape rows cycle inside their bounds: laps wrap
/// `1..=CUSTOMIZE_LAP_MAX` (the designed upper bound — authored laps
/// run 2-4, CIR-5) and opponents wrap `0..=` the authored roster count
/// — the aimap is the opponent source, so the picker cannot offer a
/// lineup the event does not wire.
#[test]
fn circuit_laps_and_opponents_rows_cycle_in_bounds() {
    let tmp = circuit_install();
    let (mut app, _store_dir) = beaten_circuit_options(&tmp);
    open_options(&mut app, "circuit0");

    focus_row(&mut app, "Laps:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[3].text, "Laps: 3");
    for _ in 0..(mm2_game::CUSTOMIZE_LAP_MAX - 3) {
        press(&mut app, KeyCode::ArrowRight);
    }
    assert_eq!(
        shell(&app).rows[3].text,
        format!("Laps: {}", mm2_game::CUSTOMIZE_LAP_MAX),
        "the picker tops out at the designed bound"
    );
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[3].text, "Laps: 1", "the top wraps to 1");
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(
        shell(&app).rows[3].text,
        format!("Laps: {}", mm2_game::CUSTOMIZE_LAP_MAX),
        "and the bottom wraps back up"
    );

    focus_row(&mut app, "Opponents:");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(
        shell(&app).rows[4].text,
        "Opponents: 0",
        "past the authored count wraps to a solo race"
    );
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[4].text, "Opponents: 2");
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[4].text, "Opponents: 1");
}

/// Changed race-shape picks ride the launched session's
/// `customization.race` and land on the built definition/roster
/// (laps on the Ordered definition, opponents on the wired lineup) —
/// a customized run stays record-ineligible (DRV-6).
#[test]
fn circuit_customized_launch_carries_the_race_picks() {
    let tmp = circuit_install();
    let (mut app, _store_dir) = beaten_circuit_options(&tmp);
    open_options(&mut app, "circuit0");

    focus_row(&mut app, "Laps:");
    press(&mut app, KeyCode::ArrowRight); // 2 → 3
    focus_row(&mut app, "Opponents:");
    press(&mut app, KeyCode::ArrowLeft); // 2 → 1
    activate_row(&mut app, "Start race");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "the customized circuit never launched: {:?}",
        phase(&app)
    );

    let config = app
        .world()
        .resource::<Session>()
        .config()
        .expect("a launched session has a config")
        .clone();
    let picks = config
        .customization
        .expect("changed picks ride the session config");
    assert_eq!(
        picks.race,
        Some(mm2_game::RaceCustomization {
            laps: 3,
            opponents: 1
        })
    );
    assert_eq!(
        mm2_game::record_eligibility(&config),
        Err(mm2_game::Ineligible::Customized)
    );
    let race = app.world().resource::<mm2_game::RaceState>();
    assert_eq!(race.definition.laps, 3, "the picked lap count bound");
    assert_eq!(
        race.definition.params.opponents, 1,
        "the param tracks the applied roster"
    );
}

/// A visit that returns the race-shape rows to their authored seeds
/// launches a default run — the picks equal the seed, so DRV-6 record
/// eligibility survives the round trip.
#[test]
fn circuit_options_returned_to_seed_launch_a_default_run() {
    let tmp = circuit_install();
    let (mut app, _store_dir) = beaten_circuit_options(&tmp);
    open_options(&mut app, "circuit0");

    focus_row(&mut app, "Laps:");
    press(&mut app, KeyCode::ArrowRight);
    press(&mut app, KeyCode::ArrowLeft); // 2 → 3 → 2
    focus_row(&mut app, "Opponents:");
    press(&mut app, KeyCode::ArrowLeft);
    press(&mut app, KeyCode::ArrowRight); // 2 → 1 → 2
    activate_row(&mut app, "Start race");
    assert!(run_until(&mut app, 12, |a| matches!(
        phase(a),
        SessionPhase::Countdown | SessionPhase::Playing
    )));

    let config = app
        .world()
        .resource::<Session>()
        .config()
        .expect("a launched session has a config");
    assert!(config.customization.is_none());
    assert_eq!(mm2_game::record_eligibility(config), Ok(()));
}

/// Imported wheel grandchildren must render in the offscreen view, while
/// showroom cameras and meshes must disappear before gameplay starts.
#[test]
fn showroom_renders_the_actual_vehicle_on_an_isolated_layer() {
    use bevy::camera::{RenderTarget, visibility::RenderLayers};
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Vehicle:");
    app.update();
    {
        let world = app.world_mut();
        let targets: Vec<_> = world
            .query::<(&Camera3d, &RenderTarget)>()
            .iter(world)
            .collect();
        assert_eq!(targets.len(), 1);
        assert!(matches!(targets[0].1, RenderTarget::Image(_)));
        let mesh_count = world.query::<&Mesh3d>().iter(world).count();
        let meshes: Vec<_> = world
            .query::<(&Mesh3d, &RenderLayers)>()
            .iter(world)
            .collect();
        assert!(meshes.len() >= 5, "body plus the four wheel meshes");
        assert_eq!(
            meshes.len(),
            mesh_count,
            "every mesh has an explicit render layer"
        );
        assert!(
            meshes
                .iter()
                .all(|(_, layers)| **layers == RenderLayers::layer(7))
        );
    }
    let before = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<Camera3d>>()
            .iter(world)
            .next()
            .unwrap()
    };
    app.update();
    assert!(
        app.world().get_entity(before).is_ok(),
        "idle preview reuses its scene"
    );
    activate_row(&mut app, "Test Car");
    focus_row(&mut app, "Blue");
    app.update();
    assert!(
        app.world().get_entity(before).is_err(),
        "paint change rebuilds the preview"
    );
    assert_eq!(
        shell(&app).vehicle.paint,
        0,
        "previewing a locked paint does not select it"
    );
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<Camera3d>>()
            .iter(world)
            .count(),
        0
    );
    assert_eq!(
        world.query::<&Mesh3d>().iter(world).count(),
        0,
        "no preview mesh leaks into the root menu"
    );
}

/// The root `Options` row opens the graphics screen, which shows the
/// look the game shipped with — shadows on High, 4x MSAA — and offers no
/// reset while nothing differs from it.
#[test]
fn options_opens_the_graphics_screen_at_the_shipped_defaults() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    focus_row(&mut app, "Options");
    assert!(shell(&app).focused_row().unwrap().enabled.is_ok());
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).screen, menu::Screen::Options);
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        rows,
        [
            "Shadows: High",
            "Anti-aliasing: 4x MSAA",
            "Text size: 100%",
            "Flashing: Normal",
            "Display: Windowed",
            "VSync: On",
            "Window size: 1280 x 720",
            "Field of view: Authored",
            "Flip recovery: Automatic",
            "Master volume: 100%",
            "Sound effects volume: 100%",
            "Commentary volume: 100%",
            "City sounds volume: 100%",
            "Reset to defaults",
            "Driving controls"
        ]
    );
    assert_eq!(
        shell(&app).rows[13].enabled,
        Err("already at the defaults".to_string())
    );
    // Esc backs out to the root with Options still focused.
    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).screen, menu::Screen::Root);
}

/// Left/Right cycle a setting, the change reaches the world's
/// `GraphicsSettings` resource and the settings file at once, and the
/// reset row restores both.
#[test]
fn option_changes_apply_persist_and_reset() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    // High -> Off wraps; the resource and the file both follow.
    focus_row(&mut app, "Shadows");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[0].text, "Shadows: Off");
    assert_eq!(
        app.world().resource::<GraphicsSettings>().shadows,
        ShadowQuality::Off
    );
    assert_eq!(GraphicsSettings::load(&path).shadows, ShadowQuality::Off);
    // Left steps back (Off -> High wraps the other way).
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[0].text, "Shadows: High");
    // Enter cycles forward like Right.
    press(&mut app, KeyCode::ArrowRight);
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[0].text, "Shadows: Low");

    focus_row(&mut app, "Anti-aliasing");
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).rows[1].text, "Anti-aliasing: Off");
    let saved = GraphicsSettings::load(&path);
    assert_eq!(saved.shadows, ShadowQuality::Low);
    assert_eq!(saved.antialiasing, Antialiasing::Off);
    assert_eq!(app.world().resource::<GraphicsSettings>(), &saved);

    // Reset is offered now, restores the defaults and disables itself.
    assert!(shell(&app).rows[13].enabled.is_ok());
    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        rows[..3],
        ["Shadows: High", "Anti-aliasing: 4x MSAA", "Text size: 100%"]
    );
    assert_eq!(
        app.world().resource::<GraphicsSettings>(),
        &GraphicsSettings::default()
    );
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert!(shell(&app).rows[13].enabled.is_err());
}

/// The volume rows step by ten percent, wrap at both ends, reach the
/// live `GraphicsSettings` and the file at once, and the reset row puts
/// them back with the graphics rows.
#[test]
fn volume_rows_step_wrap_persist_and_reset() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Master volume");
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[9].text, "Master volume: 90%");
    assert_eq!(app.world().resource::<GraphicsSettings>().audio.master, 90);
    assert_eq!(GraphicsSettings::load(&path).audio.master, 90);
    // Right from the top wraps to silence, Left from silence to the top.
    press(&mut app, KeyCode::ArrowRight);
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[9].text, "Master volume: 0%");
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[9].text, "Master volume: 100%");

    // Each bus row moves its own level and nothing else.
    focus_row(&mut app, "City sounds");
    press(&mut app, KeyCode::ArrowLeft);
    press(&mut app, KeyCode::ArrowLeft);
    let audio = GraphicsSettings::load(&path).audio;
    assert_eq!(
        (audio.master, audio.effects, audio.commentary, audio.city),
        (100, 100, 100, 80)
    );
    assert_eq!(shell(&app).rows[12].text, "City sounds volume: 80%");

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(shell(&app).rows[12].text, "City sounds volume: 100%");
}

/// The field-of-view row steps Authored/+10°/+20° and wraps, reaches the
/// live `GraphicsSettings` and the file at once, moves no other setting,
/// and the reset row puts it back.
#[test]
fn the_field_of_view_row_steps_wraps_persists_and_resets() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Field of view");
    let row = |app: &App| {
        shell(app)
            .rows
            .iter()
            .find(|r| r.text.starts_with("Field of view"))
            .map(|r| r.text.clone())
            .expect("the Options screen lists a field-of-view row")
    };
    assert_eq!(row(&app), "Field of view: Authored");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(row(&app), "Field of view: +10°");
    assert_eq!(
        app.world().resource::<GraphicsSettings>().field_of_view,
        FieldOfView::Wide
    );
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(
        GraphicsSettings::load(&path).field_of_view,
        FieldOfView::Wider
    );
    // Right from the widest wraps to authored; Left wraps back.
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(row(&app), "Field of view: Authored");
    press(&mut app, KeyCode::ArrowLeft);
    let saved = GraphicsSettings::load(&path);
    assert_eq!(saved.field_of_view, FieldOfView::Wider);
    assert_eq!(
        GraphicsSettings {
            field_of_view: FieldOfView::Authored,
            ..saved
        },
        GraphicsSettings::default(),
        "only the field of view moved"
    );

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(row(&app), "Field of view: Authored");
}

/// The flip-recovery row toggles between Automatic and Manual, reaches
/// the live `GraphicsSettings` and the file at once, moves no other
/// setting, and the reset row puts the assist back on.
#[test]
fn the_flip_recovery_row_toggles_persists_and_resets() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Flip recovery");
    let row = |app: &App| {
        shell(app)
            .rows
            .iter()
            .find(|r| r.text.starts_with("Flip recovery"))
            .map(|r| r.text.clone())
            .expect("the Options screen lists a flip-recovery row")
    };
    assert_eq!(row(&app), "Flip recovery: Automatic");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(row(&app), "Flip recovery: Manual");
    assert!(!app.world().resource::<GraphicsSettings>().auto_right);
    let saved = GraphicsSettings::load(&path);
    assert!(!saved.auto_right);
    assert_eq!(
        GraphicsSettings {
            auto_right: true,
            ..saved
        },
        GraphicsSettings::default(),
        "only the assist moved"
    );

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(row(&app), "Flip recovery: Automatic");
}

/// The text-size row steps 100/125/150% and wraps, reaches the live
/// `GraphicsSettings` and the file at once, moves no other setting, and
/// the reset row puts it back.
#[test]
fn the_text_size_row_steps_wraps_persists_and_resets() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Text size");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[2].text, "Text size: 125%");
    assert_eq!(
        app.world().resource::<GraphicsSettings>().text_size,
        TextSize::Large
    );
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[2].text, "Text size: 150%");
    assert_eq!(GraphicsSettings::load(&path).text_size, TextSize::Larger);
    // Right from the largest wraps to the shipped size; Left wraps back.
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[2].text, "Text size: 100%");
    press(&mut app, KeyCode::ArrowLeft);
    let saved = GraphicsSettings::load(&path);
    assert_eq!(saved.text_size, TextSize::Larger);
    assert_eq!(
        GraphicsSettings {
            text_size: TextSize::Normal,
            ..saved
        },
        GraphicsSettings::default(),
        "only the text size moved"
    );

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(shell(&app).rows[2].text, "Text size: 100%");
}

/// `--no-vsync` (an override for this run) must not reach the settings
/// file when the person changes another option; changing VSync itself is
/// their choice and does save.
#[test]
fn a_flag_override_stays_out_of_the_saved_file() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let disk = GraphicsSettings::default();
    disk.save(&path).unwrap();
    let run = GraphicsSettings {
        vsync: false,
        ..disk
    };
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(run, Some(path.clone()))
            .with_run_overrides(RunOverrides::between(disk, run)),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Text size");
    press(&mut app, KeyCode::ArrowRight);
    assert!(!app.world().resource::<GraphicsSettings>().vsync);
    let saved = GraphicsSettings::load(&path);
    assert_eq!(saved.text_size, TextSize::Large);
    assert!(saved.vsync, "the flag stays out of the file");

    focus_row(&mut app, "VSync");
    press(&mut app, KeyCode::Enter);
    assert!(GraphicsSettings::load(&path).vsync);
    press(&mut app, KeyCode::Enter);
    assert!(
        !GraphicsSettings::load(&path).vsync,
        "turning it off by hand is a choice"
    );
}

/// The flashing row flips Normal/Reduced from either arrow or Enter,
/// reaches the live settings and the file at once, moves nothing else,
/// and the reset row puts it back.
#[test]
fn the_flashing_row_toggles_persists_and_resets() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Flashing");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[3].text, "Flashing: Reduced");
    assert!(app.world().resource::<GraphicsSettings>().reduce_flashing);
    let saved = GraphicsSettings::load(&path);
    assert!(saved.reduce_flashing);
    assert_eq!(
        GraphicsSettings {
            reduce_flashing: false,
            ..saved
        },
        GraphicsSettings::default(),
        "only the flashing option moved"
    );
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[3].text, "Flashing: Normal");
    press(&mut app, KeyCode::Enter);
    assert!(GraphicsSettings::load(&path).reduce_flashing);

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(shell(&app).rows[3].text, "Flashing: Normal");
}

/// The display and vsync rows step from either arrow or Enter, reach
/// the live settings and the file at once, move nothing else, and the
/// reset row puts both back.
#[test]
fn the_display_and_vsync_rows_toggle_persist_and_reset() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);

    focus_row(&mut app, "Display");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[4].text, "Display: Borderless fullscreen");
    assert_eq!(
        app.world().resource::<GraphicsSettings>().display,
        DisplayMode::Fullscreen
    );
    let saved = GraphicsSettings::load(&path);
    assert_eq!(saved.display, DisplayMode::Fullscreen);
    assert_eq!(
        GraphicsSettings {
            display: DisplayMode::Windowed,
            ..saved
        },
        GraphicsSettings::default(),
        "only the display mode moved"
    );
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[4].text, "Display: Windowed");

    focus_row(&mut app, "VSync");
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).rows[5].text, "VSync: Off");
    assert!(!app.world().resource::<GraphicsSettings>().vsync);
    assert!(!GraphicsSettings::load(&path).vsync);
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[5].text, "VSync: On");
    press(&mut app, KeyCode::ArrowRight);
    assert!(!GraphicsSettings::load(&path).vsync);

    focus_row(&mut app, "Window size");
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(shell(&app).rows[6].text, "Window size: 1600 x 900");
    assert_eq!(
        app.world().resource::<GraphicsSettings>().window_size,
        WindowSize::Hd900
    );
    assert_eq!(GraphicsSettings::load(&path).window_size, WindowSize::Hd900);
    press(&mut app, KeyCode::ArrowLeft);
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(shell(&app).rows[6].text, "Window size: 2560 x 1440");
    assert_eq!(
        GraphicsSettings::load(&path).window_size,
        WindowSize::Qhd1440,
        "left from the smallest wraps to the largest"
    );

    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    assert_eq!(shell(&app).rows[5].text, "VSync: On");
    assert_eq!(shell(&app).rows[6].text, "Window size: 1280 x 720");
}

/// F23 req 6: a display change made on the Options screen is a trial.
/// The file keeps the confirmed display until it is kept, the menu's
/// own keys are dead while the banner is up (the confirming Enter is
/// not also a row activation, Esc is not also Back), and a revert hands
/// the Options screen the live value so its next edit steps from it.
#[test]
fn a_display_change_from_the_options_screen_is_a_trial_the_menu_cannot_answer() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir.path().join("saved"));
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_settings(GraphicsSettings::default(), Some(path.clone())),
    )
    .init_resource::<GraphicsSettings>()
    .insert_resource(mm2_app::settings::SettingsFile::new(Some(path.clone())))
    .init_resource::<display_trial::DisplayTrial>()
    // `DisplayTrialPlugin`'s system, ordered as there (the rig is
    // already finished, so a plugin can no longer be added).
    .add_systems(
        Update,
        display_trial::drive_display_trial
            .after(menu::menu_input)
            .after(pause::pause_input),
    );
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);
    focus_row(&mut app, "Display");
    let display = |app: &App| app.world().resource::<GraphicsSettings>().display;
    let pending = |app: &App| {
        app.world()
            .resource::<display_trial::DisplayTrial>()
            .is_pending()
    };

    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(display(&app), DisplayMode::Fullscreen);
    assert!(pending(&app), "the change opened a trial");
    assert_eq!(
        GraphicsSettings::load(&path).display,
        DisplayMode::Windowed,
        "the file keeps the confirmed display while the trial runs"
    );

    // Enter keeps — and does not also activate the focused Display row.
    press(&mut app, KeyCode::Enter);
    assert!(!pending(&app));
    assert_eq!(display(&app), DisplayMode::Fullscreen);
    assert_eq!(
        GraphicsSettings::load(&path).display,
        DisplayMode::Fullscreen
    );

    // A second change, answered with Esc: reverts to the kept display
    // and does not also leave the Options screen.
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(display(&app), DisplayMode::Windowed);
    assert!(pending(&app));
    press(&mut app, KeyCode::Escape);
    assert!(!pending(&app));
    assert_eq!(display(&app), DisplayMode::Fullscreen);
    assert_eq!(
        GraphicsSettings::load(&path).display,
        DisplayMode::Fullscreen
    );
    app.update();
    assert_eq!(shell(&app).screen, menu::Screen::Options);
    assert_eq!(shell(&app).rows[4].text, "Display: Borderless fullscreen");

    // The next edit steps from the reverted value, not the stale one.
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(display(&app), DisplayMode::Windowed);
}

/// A menu with nowhere to save (an evidence run) still applies the
/// change for the run, and a save that fails says so without refusing
/// it.
#[test]
fn unsaved_and_unsavable_settings_still_apply() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(
        app.world().resource::<GraphicsSettings>().shadows,
        ShadowQuality::Off
    );
    assert_eq!(shell(&app).status, None);

    // A path whose parent is a file cannot be created.
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let bad = blocker.path().join("settings.json");
    app.insert_resource(
        MenuData::new(None, false, None).with_settings(GraphicsSettings::default(), Some(bad)),
    );
    app.update();
    press(&mut app, KeyCode::ArrowRight);
    assert_eq!(
        app.world().resource::<GraphicsSettings>().shadows,
        ShadowQuality::Off
    );
    assert!(
        shell(&app)
            .status
            .as_deref()
            .is_some_and(|s| s.starts_with("settings not saved")),
        "{:?}",
        shell(&app).status
    );
}

/// A change made in the pause overlay edits the live resource, so the
/// main menu — reopened after the session quits — must show it, not the
/// copy it last saved itself.
#[test]
fn the_reopened_menu_shows_settings_changed_in_game() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    // Pretend a session ran: the shell is closed and the app is back on
    // the Menu phase, which is when `menu_watch` reopens it.
    app.world_mut().resource_mut::<MenuShell>().active = false;
    app.insert_resource(GraphicsSettings {
        shadows: ShadowQuality::Low,
        antialiasing: Antialiasing::X2,
        ..default()
    });
    app.update();
    assert!(shell(&app).active);
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(rows[..2], ["Shadows: Low", "Anti-aliasing: 2x MSAA"]);
}

/// F27-B.4c: Cops & Robbers is offered from the menu. A city without a
/// site pool is listed with the network gate's reason (never a button
/// that would fail at launch); a city with one opens the host's three
/// choices, which cycle in place and ride the launched session's mode.
#[test]
fn the_menu_offers_cops_and_robbers_where_the_city_can_seed_a_round() {
    use mm2_game::cnr_options::{CnrSettings, GoldMass, MatchLimit};
    use mm2_game::gold::CnrVariant;

    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();

    // No multicopwaypoints.csv: the city row is disabled and names why.
    activate_row(&mut app, "Cops & Robbers");
    let row = &shell(&app).rows[0];
    assert_eq!(row.text, "testcity");
    let reason = row.enabled.as_ref().unwrap_err();
    assert!(reason.contains("testcity"), "{reason}");
    activate_row(&mut app, "testcity");
    assert!(
        shell(&app).status.is_some(),
        "activating a disabled row says why"
    );
    assert_eq!(phase(&app), SessionPhase::Menu);
    press(&mut app, KeyCode::Escape);

    // Three authored sites make the city playable.
    let body: String = (0..3)
        .map(|i| format!("{},0,140,0,15,0,0,0,\n", 60.0 + 20.0 * i as f32))
        .collect();
    write(
        tmp.path(),
        "race/testcity/multicopwaypoints.csv",
        format!("{WAYPOINTS}{body}"),
    );
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Cops & Robbers");
    assert!(shell(&app).rows[0].enabled.is_ok());
    activate_row(&mut app, "testcity");

    // The executable's defaults first; each row cycles its own choice.
    let texts =
        |app: &App| -> Vec<String> { shell(app).rows.iter().map(|r| r.text.clone()).collect() };
    assert_eq!(
        texts(&app),
        [
            "Game: Free for all",
            "Gold weight: Weightless",
            "Limit: No limit",
            "Start match"
        ]
    );
    activate_row(&mut app, "Game:");
    activate_row(&mut app, "Gold weight:");
    activate_row(&mut app, "Gold weight:");
    activate_row(&mut app, "Limit:");
    activate_row(&mut app, "Limit:");
    assert_eq!(
        texts(&app)[..3],
        [
            "Game: Cops vs. Robbers",
            "Gold weight: Half Ton",
            "Limit: 10 minutes"
        ]
    );
    // Left steps back (and wraps) like every other value row.
    press(&mut app, KeyCode::ArrowUp);
    press(&mut app, KeyCode::ArrowUp);
    press(&mut app, KeyCode::ArrowLeft);
    assert_eq!(texts(&app)[0], "Game: Free for all");
    activate_row(&mut app, "Game:");

    activate_row(&mut app, "Start match");
    assert!(
        run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing),
        "the match never reached Playing: {:?}",
        phase(&app)
    );
    let config = app.world().resource::<Session>().config().cloned().unwrap();
    assert_eq!(
        config.mode,
        SessionMode::CopsAndRobbers(CnrSettings {
            variant: CnrVariant::CopsVsRobbers,
            gold_mass: GoldMass::HalfTon,
            limit: MatchLimit::Minutes(10),
        })
    );
    assert!(
        app.world()
            .get_resource::<mm2_app::cnr::CnrHost>()
            .is_some(),
        "the launched mode builds the match"
    );
}

/// F27-B.4c (rematch leg): a decided single-seat match opens the
/// match-over screen instead of idling on a frozen HUD, and its *Play
/// again* row begins a fresh generation with a fresh match — the same
/// restart a race's results use, so nothing of the old match survives.
#[test]
fn a_decided_cops_and_robbers_match_offers_play_again() {
    use mm2_app::cnr::{CnrHost, end_decided_match};
    use mm2_app::results::ResultsUi;

    let tmp = install();
    let body: String = (0..3)
        .map(|i| format!("{},0,140,0,15,0,0,0,\n", 60.0 + 20.0 * i as f32))
        .collect();
    write(
        tmp.path(),
        "race/testcity/multicopwaypoints.csv",
        format!("{WAYPOINTS}{body}"),
    );
    let mut app = menu_app(tmp.path(), None);
    app.add_systems(Update, end_decided_match.after(session::drive_session));
    app.update();
    activate_row(&mut app, "Cops & Robbers");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Limit:"); // 5 minutes
    activate_row(&mut app, "Start match");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));
    let first = app.world().resource::<Session>().generation();
    assert!(
        app.world().resource::<CnrHost>().game.outcome().is_none(),
        "a fresh match is undecided"
    );

    // Run the match clock out; the screen opens on the next step.
    {
        let mut host = app.world_mut().resource_mut::<CnrHost>();
        for _ in 0..(5 * 60 * mm2_game::RACE_TICK_HZ) {
            host.game.tick();
        }
        assert!(host.game.outcome().is_some());
    }
    app.update();
    assert_eq!(phase(&app), SessionPhase::Results);
    app.update();
    let texts = |app: &mut App| -> Vec<String> {
        let mut q = app.world_mut().query_filtered::<&Text, With<ResultsUi>>();
        q.iter(app.world()).map(|t| t.0.clone()).collect()
    };
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t == "Cops & Robbers"), "{shown:?}");
    assert!(
        shown.iter().any(|t| t.contains("Play again")),
        "the restart row is named for the match: {shown:?}"
    );
    assert!(
        shown.iter().any(|t| t.starts_with("time - played 5:00")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|t| t.contains("Restart race")),
        "{shown:?}"
    );

    // Play again: down to the row, activate; a fresh generation plays
    // a fresh, undecided match.
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::Enter);
    assert!(
        run_until(&mut app, 24, |a| {
            phase(a) == SessionPhase::Playing
                && a.world().resource::<Session>().generation() != first
        }),
        "the rematch never started: {:?}",
        phase(&app)
    );
    let host = app.world().resource::<CnrHost>();
    assert!(host.game.outcome().is_none());
    assert_eq!(host.game.elapsed_ticks(), 0, "the clock starts over");
}

/// Open the Controls screen from the root and return the file it saves
/// to (inside `dir`).
fn open_controls(app: &mut App, dir: &Path) -> std::path::PathBuf {
    let path = mm2_app::controls::controls_path(dir);
    app.insert_resource(
        MenuData::new(None, false, None)
            .with_controls(ControlSettings::default(), Some(path.clone())),
    );
    app.update();
    focus_row(app, "Options");
    press(app, KeyCode::Enter);
    focus_row(app, "Driving controls");
    press(app, KeyCode::Enter);
    assert_eq!(shell(app).screen, menu::Screen::Controls);
    path
}

/// The Controls screen lists each action's two keys (the alternate on
/// the side entry), the stick tuning rows and a reset that is disabled
/// at the shipped map.
#[test]
fn the_controls_screen_lists_every_binding_and_tuning_row() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    open_controls(&mut app, dir.path());
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        rows,
        [
            "Accelerate: KeyW",
            "Brake / reverse: KeyS",
            "Steer left: KeyA",
            "Steer right: KeyD",
            "Handbrake: Space",
            "Shift up (manual): KeyG",
            "Shift down (manual): KeyB",
            "Transmission: Automatic",
            "Stick deadzone: 5%",
            "Trigger deadzone: 5%",
            "Steering sensitivity: 1.00x",
            "Invert stick steering: Off",
            "Reset to defaults",
            "Gamepad buttons",
        ]
    );
    let alts: Vec<Option<String>> = shell(&app)
        .rows
        .iter()
        .map(|r| r.side.as_ref().map(|s| s.text.clone()))
        .collect();
    assert_eq!(alts[0].as_deref(), Some("Alt: ArrowUp"));
    assert_eq!(alts[4].as_deref(), Some("Alt: -"));
    assert_eq!(alts[6].as_deref(), Some("Alt: -"), "shift down");
    assert_eq!(alts[7], None, "tuning rows have no alternate");
    assert!(shell(&app).rows[12].enabled.is_err());
    // Esc leaves for the graphics screen with the Controls row focused.
    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).screen, menu::Screen::Options);
}

/// Activate listens, the next key binds, the new map reaches the
/// `ControlSettings` resource and `controls.json` at once, and a
/// reload from the file gives the same map.
#[test]
fn a_captured_key_rebinds_the_action_and_persists() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let path = open_controls(&mut app, dir.path());

    focus_row(&mut app, "Accelerate");
    press(&mut app, KeyCode::Enter);
    assert!(shell(&app).capture.is_some());
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("press the new key")
    );
    press(&mut app, KeyCode::KeyP);
    assert_eq!(shell(&app).capture, None);
    assert_eq!(shell(&app).rows[0].text, "Accelerate: KeyP");
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("Accelerate is now KeyP")
    );

    let live = app.world().resource::<ControlSettings>();
    assert_eq!(live.key_at(DriveAction::Throttle, 0), Some(KeyCode::KeyP));
    assert_eq!(
        live.key_at(DriveAction::Throttle, 1),
        Some(KeyCode::ArrowUp)
    );
    assert_eq!(&ControlSettings::load(&path), live);
    // The reset row woke up; it restores the shipped map everywhere.
    assert!(shell(&app).rows[12].enabled.is_ok());
    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.world().resource::<ControlSettings>(),
        &ControlSettings::default()
    );
    assert_eq!(ControlSettings::load(&path), ControlSettings::default());
}

/// Open the gamepad-buttons screen from the root, returning the controls
/// file it saves to and the pad that will press buttons.
fn open_pad_buttons(app: &mut App, dir: &Path) -> (std::path::PathBuf, Entity) {
    let path = open_controls(app, dir);
    let pad = spawn_pad(app);
    focus_row(app, "Gamepad buttons");
    press(app, KeyCode::Enter);
    assert_eq!(shell(app).screen, menu::Screen::PadButtons);
    (path, pad)
}

/// F23-A.4: the gamepad-buttons screen lists every pad action with its
/// shipped button, and a reset that is disabled at the shipped map.
#[test]
fn the_pad_screen_lists_every_pad_action_with_its_button() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    open_pad_buttons(&mut app, dir.path());
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(
        rows,
        [
            "Handbrake: South",
            "Shift up (manual): RightBumper",
            "Shift down (manual): LeftBumper",
            "Change camera: RightStick",
            "Cockpit view: West",
            "Rear-view mirror: East",
            "Reset vehicle: North",
            "Horn / siren: LeftStick",
            "Map view: Select",
            "Map zoom: DPadLeft",
            "Map orientation: DPadRight",
            "Driving HUD: DPadUp",
            "Opponent indicators: DPadDown",
            "Previous target: LeftBumper",
            "Next target: RightBumper",
            "Reset gamepad buttons",
        ]
    );
    assert!(
        shell(&app).rows[15].enabled.is_err(),
        "nothing to reset yet"
    );
    // Esc returns to the Controls screen on the row that opened this one.
    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).screen, menu::Screen::Controls);
    assert!(
        shell(&app).rows[shell(&app).focus]
            .text
            .contains("Gamepad buttons")
    );
}

/// Listening takes the next pad button: nav buttons bind instead of
/// navigating, a button another action holds is refused naming its
/// owner (and listening continues), `Start` cancels, X clears an action
/// to free a button, and every change reaches the live resource and
/// `controls.json`.
#[test]
fn a_captured_pad_button_rebinds_the_action_and_persists() {
    use mm2_app::pad_map::PadAction;

    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let (path, pad) = open_pad_buttons(&mut app, dir.path());

    focus_row(&mut app, "Handbrake");
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).pad_capture, Some(PadAction::Handbrake));
    assert_eq!(shell(&app).capture, None);
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("press the new button")
    );

    // East is Back on every menu — here it is just a candidate, and a
    // taken one.
    pad_press(&mut app, pad, GamepadButton::East);
    assert_eq!(shell(&app).screen, menu::Screen::PadButtons);
    assert_eq!(shell(&app).pad_capture, Some(PadAction::Handbrake));
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("already bound to Rear-view mirror"),
        "{:?}",
        shell(&app).status
    );
    // `Start` is app-owned: it cancels rather than binds.
    pad_press(&mut app, pad, GamepadButton::Start);
    assert_eq!(shell(&app).pad_capture, None);
    assert_eq!(shell(&app).status.as_deref(), Some("rebinding cancelled"));
    assert!(
        app.world().get_resource::<ControlSettings>().is_none() && !path.exists(),
        "refusals and cancels publish and save nothing"
    );

    // Free North (X clears), then give it to the handbrake.
    focus_row(&mut app, "Reset vehicle");
    press(&mut app, KeyCode::KeyX);
    assert_eq!(shell(&app).rows[6].text, "Reset vehicle: -");
    focus_row(&mut app, "Handbrake");
    press(&mut app, KeyCode::Enter);
    pad_press(&mut app, pad, GamepadButton::North);
    assert_eq!(shell(&app).pad_capture, None);
    assert_eq!(shell(&app).rows[0].text, "Handbrake: North");
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("Handbrake is now North")
    );
    let live = app.world().resource::<ControlSettings>();
    assert_eq!(
        live.pad.button(PadAction::Handbrake),
        Some(GamepadButton::North)
    );
    assert_eq!(live.pad.button(PadAction::Reset), None);
    assert_eq!(&ControlSettings::load(&path), live, "saved as it stands");

    // Clearing an action that is already clear says so.
    focus_row(&mut app, "Reset vehicle");
    press(&mut app, KeyCode::KeyX);
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("no button is bound")
    );

    // The page's reset restores the shipped buttons.
    focus_row(&mut app, "Reset gamepad");
    assert!(shell(&app).rows[15].enabled.is_ok());
    press(&mut app, KeyCode::Enter);
    assert!(shell(&app).rows[15].enabled.is_err());
    let live = app.world().resource::<ControlSettings>();
    assert_eq!(live.pad, mm2_app::pad_map::PadMap::default());
}

/// The mouse's right button and the keyboard's Esc cancel a pad capture
/// the way they cancel a key capture; hovering and nav keys are inert.
#[test]
fn escape_cancels_a_pad_capture_and_nav_keys_stay_inert() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    open_pad_buttons(&mut app, dir.path());
    focus_row(&mut app, "Map zoom");
    let focus = shell(&app).focus;
    press(&mut app, KeyCode::Enter);
    assert!(shell(&app).pad_capture.is_some());
    press(&mut app, KeyCode::ArrowDown);
    assert_eq!(shell(&app).focus, focus, "no navigation while listening");
    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).pad_capture, None);
    assert_eq!(shell(&app).screen, menu::Screen::PadButtons);
    assert!(app.world().get_resource::<ControlSettings>().is_none());
}

/// The Transmission row flips automatic/manual in place, reaches the
/// live resource and `controls.json`, and the manual shift keys are
/// rebindable like any other action.
#[test]
fn the_transmission_row_switches_policy_and_the_shift_keys_rebind() {
    use mm2_app::controls::TransmissionPolicy;

    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let path = open_controls(&mut app, dir.path());
    focus_row(&mut app, "Transmission");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        shell(&app).rows[7].text,
        "Transmission: Manual",
        "the row shows the new policy"
    );
    let live = app.world().resource::<ControlSettings>().clone();
    assert_eq!(live.transmission, TransmissionPolicy::Manual);
    assert_eq!(ControlSettings::load(&path), live);

    focus_row(&mut app, "Shift up");
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).capture, Some((DriveAction::ShiftUp, 0)));
    press(&mut app, KeyCode::KeyU);
    let live = app.world().resource::<ControlSettings>().clone();
    assert_eq!(live.key_at(DriveAction::ShiftUp, 0), Some(KeyCode::KeyU));
    assert_eq!(ControlSettings::load(&path), live);

    // The reset row wakes up and returns the policy to automatic too.
    focus_row(&mut app, "Reset");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.world().resource::<ControlSettings>().transmission,
        TransmissionPolicy::Automatic
    );
}

/// The alternate key rebinds through the side entry, independently of
/// the primary.
#[test]
fn the_alternate_slot_rebinds_through_the_side_entry() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    open_controls(&mut app, dir.path());
    focus_row(&mut app, "Handbrake");
    press(&mut app, KeyCode::ArrowRight);
    assert!(shell(&app).side);
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).capture, Some((DriveAction::Handbrake, 1)));
    press(&mut app, KeyCode::KeyJ);
    let live = app.world().resource::<ControlSettings>();
    assert_eq!(live.key_at(DriveAction::Handbrake, 0), Some(KeyCode::Space));
    assert_eq!(live.key_at(DriveAction::Handbrake, 1), Some(KeyCode::KeyJ));
}

/// While listening, nav keys bind instead of moving focus; a key
/// another action owns, a reserved key and an unbindable key are all
/// refused with the reason and the screen keeps listening; Esc cancels
/// without changing anything.
#[test]
fn capture_refuses_bad_keys_keeps_listening_and_cancels_on_escape() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let path = open_controls(&mut app, dir.path());
    focus_row(&mut app, "Accelerate");
    press(&mut app, KeyCode::Enter);

    // KeyS belongs to Brake: refused, names the owner, still listening,
    // and focus did not follow the S (a nav key outside capture).
    press(&mut app, KeyCode::KeyS);
    assert_eq!(shell(&app).focus, 0);
    assert!(shell(&app).capture.is_some());
    let status = shell(&app).status.clone().unwrap();
    assert!(
        status.contains("KeyS") && status.contains("Brake / reverse"),
        "{status}"
    );
    // Reserved (camera C) and unbindable (F1) keys likewise.
    press(&mut app, KeyCode::KeyC);
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("in-game control")
    );
    press(&mut app, KeyCode::F1);
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("cannot be used")
    );
    assert!(shell(&app).capture.is_some());
    // Hover/click commands are inert while listening.
    app.world_mut()
        .resource_mut::<MenuShell>()
        .pending
        .push(menu::MenuCommand::FocusAt(3));
    app.update();
    assert_eq!(shell(&app).focus, 0);
    assert!(shell(&app).capture.is_some());

    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).capture, None);
    assert_eq!(shell(&app).status.as_deref(), Some("rebinding cancelled"));
    assert_eq!(shell(&app).screen, menu::Screen::Controls);
    assert!(
        app.world()
            .get_resource::<ControlSettings>()
            .is_none_or(|c| *c == ControlSettings::default())
    );
    assert!(!path.exists(), "nothing was saved");
}

/// X clears a key slot; an action's last key stays, with the reason.
#[test]
fn clearing_a_key_keeps_the_last_one() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let path = open_controls(&mut app, dir.path());
    focus_row(&mut app, "Accelerate");
    press(&mut app, KeyCode::ArrowRight);
    press(&mut app, KeyCode::KeyX);
    assert_eq!(shell(&app).rows[0].side.as_ref().unwrap().text, "Alt: -");
    assert_eq!(
        ControlSettings::load(&path).key_at(DriveAction::Throttle, 1),
        None
    );
    assert!(shell(&app).status.as_deref().unwrap().contains("cleared"));

    // Back on the primary, the only key left cannot be cleared.
    press(&mut app, KeyCode::ArrowLeft);
    press(&mut app, KeyCode::KeyX);
    assert_eq!(shell(&app).rows[0].text, "Accelerate: KeyW");
    assert!(
        shell(&app)
            .status
            .as_deref()
            .unwrap()
            .contains("at least one key")
    );
}

/// The tuning rows step in place, reach the resource and the file, and
/// leaving the screen mid-capture drops the pending capture.
#[test]
fn tuning_rows_apply_and_persist_and_leaving_drops_a_capture() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let mut app = menu_app(tmp.path(), None);
    let path = open_controls(&mut app, dir.path());
    focus_row(&mut app, "Stick deadzone");
    press(&mut app, KeyCode::ArrowRight);
    focus_row(&mut app, "Steering sensitivity");
    press(&mut app, KeyCode::ArrowRight);
    focus_row(&mut app, "Invert");
    press(&mut app, KeyCode::Enter);
    let rows: Vec<String> = shell(&app).rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(rows[8], "Stick deadzone: 10%");
    assert_eq!(rows[10], "Steering sensitivity: 1.25x");
    assert_eq!(rows[11], "Invert stick steering: On");
    let live = app.world().resource::<ControlSettings>().clone();
    assert_eq!(live.steer_deadzone, 0.10);
    assert!(live.invert_steering);
    assert_eq!(ControlSettings::load(&path), live);

    // Start a capture, then back out with the pad's East button path:
    // Back cancels the capture first and a second Back leaves.
    focus_row(&mut app, "Handbrake");
    press(&mut app, KeyCode::Enter);
    assert!(shell(&app).capture.is_some());
    app.world_mut()
        .resource_mut::<MenuShell>()
        .pending
        .push(menu::MenuCommand::Back);
    app.update();
    assert_eq!(shell(&app).capture, None);
    assert_eq!(shell(&app).screen, menu::Screen::Controls);
    press(&mut app, KeyCode::Escape);
    assert_eq!(shell(&app).screen, menu::Screen::Options);
}

/// A connected pad with no events behind it — the mocking surface bevy
/// documents (`digital_mut`/`analog_mut`); these apps run no
/// `InputPlugin`, so edges are managed by hand like the keyboard's.
fn spawn_pad(app: &mut App) -> Entity {
    app.world_mut().spawn(Gamepad::default()).id()
}

/// Press `button` on `pad` for exactly one update.
fn pad_press(app: &mut App, pad: Entity, button: GamepadButton) {
    app.world_mut()
        .get_mut::<Gamepad>(pad)
        .unwrap()
        .digital_mut()
        .press(button);
    app.update();
    app.world_mut()
        .get_mut::<Gamepad>(pad)
        .unwrap()
        .digital_mut()
        .reset_all();
}

fn set_pad_stick_y(app: &mut App, pad: Entity, y: f32) {
    app.world_mut()
        .get_mut::<Gamepad>(pad)
        .unwrap()
        .analog_mut()
        .set(GamepadAxis::LeftStickY, y);
    app.update();
}

/// F23-AC03 hot-plug/ownership leg on the main menu: an idle first pad
/// (a spare controller, a wheel that registers as a gamepad) must not
/// shadow the pad the player holds; unplugging it, or every pad, leaves
/// navigation working, and a pad plugged in later answers.
#[test]
fn any_connected_pad_navigates_the_main_menu_through_hot_plug() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    let idle = spawn_pad(&mut app);
    let held = spawn_pad(&mut app);
    let start = shell(&app).focus;

    pad_press(&mut app, held, GamepadButton::DPadDown);
    assert_eq!(shell(&app).focus, start + 1, "the second pad navigates");
    // Stick edges come from the pad that is actually deflected.
    set_pad_stick_y(&mut app, held, 1.0);
    assert_eq!(shell(&app).focus, start, "the second pad's stick moves up");
    set_pad_stick_y(&mut app, held, 1.0);
    assert_eq!(
        shell(&app).focus,
        start,
        "a held stick does not run the list"
    );
    set_pad_stick_y(&mut app, held, 0.0);

    // The idle pad is unplugged mid-menu: the held one still answers.
    app.world_mut().despawn(idle);
    pad_press(&mut app, held, GamepadButton::DPadDown);
    assert_eq!(shell(&app).focus, start + 1, "survivor keeps navigating");

    // Every pad gone: the keyboard is unaffected, and a pad plugged in
    // afterwards works without a restart of the screen.
    app.world_mut().despawn(held);
    press(&mut app, KeyCode::ArrowDown);
    assert_eq!(shell(&app).focus, start + 2);
    let late = spawn_pad(&mut app);
    pad_press(&mut app, late, GamepadButton::DPadUp);
    assert_eq!(shell(&app).focus, start + 1, "a newly plugged pad answers");

    // Accept/Back come from any pad too: Options opens, East leaves.
    let other = spawn_pad(&mut app);
    focus_row(&mut app, "Options");
    pad_press(&mut app, other, GamepadButton::South);
    assert_eq!(shell(&app).screen, menu::Screen::Options);
    pad_press(&mut app, other, GamepadButton::East);
    assert_ne!(shell(&app).screen, menu::Screen::Options);
}

/// A stick held down when its pad is unplugged must not leave a stale
/// latch: the next pad pushing down fires its own edge.
#[test]
fn a_stick_held_through_an_unplug_does_not_poison_the_edge_latch() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    let first = spawn_pad(&mut app);
    let start = shell(&app).focus;
    set_pad_stick_y(&mut app, first, -1.0);
    assert_eq!(shell(&app).focus, start + 1);
    app.world_mut().despawn(first);
    app.update();

    let second = spawn_pad(&mut app);
    set_pad_stick_y(&mut app, second, -1.0);
    assert_eq!(
        shell(&app).focus,
        start + 2,
        "the new pad's push is a fresh edge"
    );
}

/// The pause overlay answers any connected pad — Down moves the focus
/// and `Start` on the second pad resumes.
#[test]
fn the_pause_menu_answers_any_pad() {
    let tmp = install();
    let mut app = menu_app(tmp.path(), None);
    app.update();
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));

    let _idle = spawn_pad(&mut app);
    let held = spawn_pad(&mut app);
    press(&mut app, KeyCode::Escape);
    assert_eq!(phase(&app), SessionPhase::Paused);
    let focus = |a: &App| a.world().resource::<PauseMenu>().focus;
    assert_eq!(focus(&app), 0);
    pad_press(&mut app, held, GamepadButton::DPadDown);
    assert_eq!(focus(&app), 1, "the second pad moves the pause focus");
    pad_press(&mut app, held, GamepadButton::Start);
    assert_eq!(phase(&app), SessionPhase::Playing, "Start resumes");
}

/// A rebind made from the pause overlay is the map the main menu's
/// Controls screen shows after quitting, and a later menu edit keeps it
/// (the menu's copy re-syncs from the live resource instead of
/// overwriting the pause edit). A pad cancels a pending capture without
/// binding a button's side effect.
#[test]
fn a_pause_rebind_survives_into_the_main_menu_controls_screen() {
    let tmp = install();
    let dir = tempfile::tempdir().unwrap();
    let path = mm2_app::controls::controls_path(dir.path());
    let mut app = menu_app(tmp.path(), None);
    app.insert_resource(ControlSettings::default())
        .insert_resource(mm2_app::controls::ControlsSave(Some(path.clone())))
        .insert_resource(GraphicsSettings::default())
        .insert_resource(
            MenuData::new(None, false, None)
                .with_controls(ControlSettings::default(), Some(path.clone())),
        );
    app.update();
    activate_row(&mut app, "Cruise");
    activate_row(&mut app, "testcity");
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Playing));

    // Pause -> Options -> Driving controls, then listen on Accelerate.
    let pad = spawn_pad(&mut app);
    press(&mut app, KeyCode::Escape);
    assert_eq!(phase(&app), SessionPhase::Paused);
    for _ in 0..2 {
        press(&mut app, KeyCode::ArrowDown);
    }
    press(&mut app, KeyCode::Enter);
    for _ in 0..14 {
        press(&mut app, KeyCode::ArrowDown);
    }
    press(&mut app, KeyCode::Enter);
    let pause = |a: &App| {
        let p = a.world().resource::<PauseMenu>();
        (p.page, p.capture.is_some())
    };
    assert_eq!(pause(&app), (mm2_app::pause::PausePage::Controls, false));
    press(&mut app, KeyCode::Enter);
    assert_eq!(pause(&app), (mm2_app::pause::PausePage::Controls, true));
    // The pad's East backs out of the listen and nothing is bound; its
    // D-pad cannot move the focus while listening.
    pad_press(&mut app, pad, GamepadButton::DPadDown);
    assert_eq!(app.world().resource::<PauseMenu>().focus, 0);
    pad_press(&mut app, pad, GamepadButton::East);
    assert_eq!(pause(&app), (mm2_app::pause::PausePage::Controls, false));
    assert_eq!(
        app.world().resource::<ControlSettings>(),
        &ControlSettings::default()
    );
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::KeyP);
    assert_eq!(
        app.world()
            .resource::<ControlSettings>()
            .key_at(DriveAction::Throttle, 0),
        Some(KeyCode::KeyP)
    );

    // Back out two pages and quit to the menu (the pause rows' Quit).
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::Enter);
    assert!(run_until(&mut app, 30, |a| {
        phase(a) == SessionPhase::Menu && shell(a).active
    }));

    // The main menu's Controls screen starts from the pause edit...
    focus_row(&mut app, "Options");
    press(&mut app, KeyCode::Enter);
    focus_row(&mut app, "Driving controls");
    press(&mut app, KeyCode::Enter);
    assert_eq!(shell(&app).screen, menu::Screen::Controls);
    assert_eq!(shell(&app).rows[0].text, "Accelerate: KeyP");
    // ...and a menu edit saves the pause edit along with it.
    focus_row(&mut app, "Invert stick steering");
    press(&mut app, KeyCode::Enter);
    let saved = ControlSettings::load(&path);
    assert!(saved.invert_steering);
    assert_eq!(saved.key_at(DriveAction::Throttle, 0), Some(KeyCode::KeyP));
    assert_eq!(&saved, app.world().resource::<ControlSettings>());
}

/// The race systems the binary runs after the physics step, plus the
/// F16-B result consumer — `menu_app` stops at the menu/session loop,
/// so the journey adds exactly what a played event needs on top.
fn add_race_and_progression(app: &mut App) {
    app.add_message::<mm2_game::RaceStarted>()
        .add_message::<mm2_game::BangerStateChanged>()
        .add_systems(
            FixedLast,
            (
                mm2_app::contracts::collect_impacts,
                mm2_app::contracts::publish_vehicle_telemetry,
                mm2_app::race::reanchor_teleported_participants,
                mm2_app::race::advance_race,
            )
                .chain(),
        )
        .add_systems(Update, mm2_app::progression::record_session_results);
}

/// Hold full throttle on the player's car (after a short settle) until
/// the session leaves `Playing`/`Countdown` or `max` updates pass.
fn drive_until_results(app: &mut App, max: usize) -> bool {
    for f in 0..max {
        let car = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerVehicle>>()
            .iter(app.world())
            .next();
        if let Some(mut input) =
            car.and_then(|c| app.world_mut().get_mut::<mm2_vehicle::VehicleInput>(c))
        {
            *input = mm2_vehicle::VehicleInput {
                throttle: if f > 200 { 1.0 } else { 0.0 },
                ..default()
            };
        }
        app.update();
        if phase(app) == SessionPhase::Results {
            return true;
        }
    }
    false
}

/// F31-B.1 / F31-AC03 (synthetic journey): one scripted run over the
/// production menu → session → results → reward path with no direct
/// state writes — a driver is named on the menu, a reward-gated paint
/// and a gated race are shown locked, the first race is launched from
/// the Events list and driven to its authored finish, the results
/// screen restarts it into a fresh generation, quitting returns to the
/// menu where the earned paint is open, the race is badged won and the
/// gated race's lock no longer names it, and a fresh app over the same profile directory (a
/// relaunch) sees the same. The saved profile is read back from disk.
#[test]
fn a_named_driver_races_earns_a_reward_and_finds_it_after_a_relaunch() {
    use mm2_app::results::ResultsUi;

    let tmp = install();
    // Gates faced down the +x lane (heading -90) so a straight drive
    // sweeps each one; the shared install's heading-0 gates face +z.
    let mut course = WAYPOINTS.to_string();
    for x in [60.0f32, 110.0, 140.0, 165.0, 180.0] {
        course.push_str(&format!("{x},0,140,-90,15,0,0,0,\n"));
    }
    write(tmp.path(), "race/testcity/race0waypoints.csv", course);
    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let mut app = menu_app(tmp.path(), Some(store.clone()));
    add_race_and_progression(&mut app);
    app.update();

    // Profile: the first-run driver is named on the entry screen.
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "New driver");
    type_text(&mut app, "Ada");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Escape);
    app.update();
    let id = app
        .world()
        .get_resource::<ActiveProfile>()
        .expect("naming a driver binds it")
        .profile
        .id
        .clone();

    // Before: the earned paint and the gated race are both shut.
    activate_row(&mut app, "Vehicle:");
    activate_row(&mut app, "Test Car");
    let blue = |app: &App| {
        shell(app)
            .rows
            .iter()
            .find(|r| r.text.contains("Blue"))
            .map(|r| r.enabled.clone())
            .expect("the Blue paint is listed")
    };
    assert!(blue(&app).unwrap_err().contains("locked"));
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    let race3 = |app: &App| {
        shell(app)
            .rows
            .iter()
            .find(|r| r.text.contains("race3"))
            .map(|r| r.enabled.clone())
            .expect("race3 is listed")
    };
    assert!(race3(&app).is_err(), "race3 starts gated behind race0");

    // Load + race: launch race0 and drive the authored course.
    activate_row(&mut app, "race0");
    assert!(
        run_until(&mut app, 12, |a| matches!(
            phase(a),
            SessionPhase::Countdown | SessionPhase::Playing
        )),
        "race0 never launched: {:?}",
        phase(&app)
    );
    let first = app.world().resource::<Session>().generation();
    assert!(
        drive_until_results(&mut app, 1500),
        "the drive never reached the results screen: {:?}",
        phase(&app)
    );

    // Outcome + reward: the finish is saved before any menu is shown.
    let saved = store.load(&id).unwrap().profile;
    let beaten = |p: &mm2_game::PlayerProfile| {
        p.progress
            .events
            .iter()
            .find(|r| r.key.stem == "race0")
            .is_some_and(|r| r.is_beaten())
    };
    assert!(beaten(&saved), "the finish is recorded on the driver");
    assert!(
        saved.progress.unlocks.contains("paint:vpt:1"),
        "{:?}",
        saved.progress
    );
    app.update();
    let shown: Vec<String> = {
        let mut q = app.world_mut().query_filtered::<&Text, With<ResultsUi>>();
        q.iter(app.world()).map(|t| t.0.clone()).collect()
    };
    assert!(
        shown.iter().any(|t| t.contains("Continue to menu"))
            && shown.iter().any(|t| t.contains("Restart race")),
        "{shown:?}"
    );

    // Restart: the results row begins a fresh generation, which is
    // then left through the pause menu.
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::Enter);
    assert!(
        run_until(&mut app, 24, |a| {
            matches!(phase(a), SessionPhase::Countdown | SessionPhase::Playing)
                && a.world().resource::<Session>().generation() != first
        }),
        "the restart never began: {:?}",
        phase(&app)
    );
    quit_session(&mut app);
    assert!(run_until(&mut app, 12, |a| phase(a) == SessionPhase::Menu));
    app.update();

    // Menu again: the badge, the open race and the earned paint.
    let state = |app: &App| {
        let won = shell(app)
            .rows
            .iter()
            .find(|r| r.text.contains("race0"))
            .and_then(|r| r.won);
        (won.map(|w| w.amateur), race3(app).err())
    };
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    let (badge, gate) = state(&app);
    assert_eq!(badge, Some(true), "the finished race is badged won");
    let gate = gate.expect("race3 also needs the rest of the first set");
    assert!(
        !gate.contains("race0") && gate.contains("race1"),
        "the gate now names only what is left: {gate}"
    );
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Vehicle:");
    activate_row(&mut app, "Test Car");
    assert!(blue(&app).is_ok(), "the earned paint is open");

    // Relaunch: a fresh app over the same profile directory remembers.
    drop(app);
    let mut app = menu_app(
        tmp.path(),
        Some(ProfileStore::open(store_dir.path()).unwrap()),
    );
    app.update();
    activate_row(&mut app, "Driver:");
    activate_row(&mut app, "Ada");
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Vehicle:");
    activate_row(&mut app, "Test Car");
    assert!(blue(&app).is_ok(), "the paint survives a relaunch");
    press(&mut app, KeyCode::Escape);
    press(&mut app, KeyCode::Escape);
    activate_row(&mut app, "Events");
    activate_row(&mut app, "testcity");
    activate_row(&mut app, "Checkpoint");
    let (badge, gate) = state(&app);
    assert_eq!(badge, Some(true), "the win survives a relaunch");
    assert!(
        gate.is_some_and(|g| !g.contains("race0")),
        "so does the narrowed gate"
    );
}

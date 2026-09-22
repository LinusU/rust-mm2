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
use mm2_app::menu::{self, MenuCamera, MenuData, MenuShell, MenuUi};
use mm2_app::pause::{self, PauseMenu};
use mm2_app::profile::ActiveProfile;
use mm2_app::results::{self, ResultsMenu};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
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
                // Same ordering contract as the binary: pause owns
                // `Paused`, running between the intent reader (which
                // ignores `Paused`) and the driver.
                pause::pause_input
                    .after(session::session_control_input)
                    .before(session::drive_session),
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
                    menu::menu_mouse,
                    menu::menu_input,
                    menu::menu_present,
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
        assert_eq!(menu_roots(&mut app), 0, "menu drew mid-restart");
        assert_eq!(menu_cameras(&mut app), 0, "menu camera mid-restart");
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

    // The drawn row entities map 1:1 onto the shell's rows.
    let world = app.world_mut();
    let count = world.query::<&menu::MenuRow>().iter(world).count();
    assert_eq!(count, shell(&app).rows.len());

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

    let options = shell(&app)
        .rows
        .iter()
        .position(|r| r.text == "Options")
        .expect("the root screen lists Options");
    cursor_to(&mut app, 200.0, row_y(options));
    click(&mut app, MouseButton::Left);
    assert!(matches!(shell(&app).screen, menu::Screen::Root));
    assert_eq!(shell(&app).focus, options);
    assert_eq!(
        shell(&app).status.as_deref(),
        Some("not implemented yet (F23)"),
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
            .contains("crash course"),
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

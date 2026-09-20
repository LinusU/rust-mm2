//! `mm2` — executable shell for the MM2-inspired engine.
//!
//! Usage:
//!   cargo run -p mm2_app --bin mm2 -- --dev-world
//!   cargo run -p mm2_app --bin mm2 -- --dev-world --mods examples/mods
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path "/path/to/Midtown Madness 2" [--city london]
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path <dir> --mods <dir> --vehicle-config <toml>
//!
//! Smoke modes print `smoke=<kind> … status=<pass|fail|unavailable>`
//! records (see `mm2_app::smoke`) and exit 0/3/4 respectively; usage
//! errors exit 2.

use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use clap::Parser;
use mm2_app::{camera, car_visual, city, dev_world, input, smoke};
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_content::{VehicleCatalog, VehicleDef};
use mm2_game::{
    CameraPose, DevOverrides, Mm2Vfs, PlayerVehicle, Session, SessionConfig, SessionEntity,
    SessionPhase, VehicleSelection, WorldMode, advance_session_tick,
};
use mm2_vehicle::{ResetVehicle, VehicleConfig, VehicleDebugEnabled, VehiclePlugin};
use tracing::{error, info, warn};

use camera::{CameraMode, ChaseCamera, FreeCamera};
use car_visual::{WheelMount, WheelSpin};

#[derive(Parser, Debug)]
#[command(name = "mm2", about = "MM2-inspired open engine — development build")]
struct Cli {
    /// Spawn the synthetic development playground (no MM2 data needed).
    #[arg(long)]
    dev_world: bool,

    /// Path to a Midtown Madness 2 installation (directory containing the
    /// .ar archives and/or loose files).
    #[arg(long)]
    mm2_path: Option<PathBuf>,

    /// Directory containing mod folders (each with a mod.toml).
    #[arg(long)]
    mods: Option<PathBuf>,

    /// City to load through the VFS (install or mods). Defaults to
    /// `london` when --mm2-path is given. A specifically requested city
    /// the VFS cannot provide is a hard failure, never a silent dev
    /// world.
    #[arg(long)]
    city: Option<String>,

    /// Stock/modded vehicle id or unique display-name alias to load
    /// (`--list-cars` shows the roster). Requires `--mm2-path` or mods
    /// providing vehicle data.
    #[arg(long)]
    car: Option<String>,

    /// Paint variant for the selected vehicle (zero-based index).
    #[arg(long, default_value_t = 0)]
    paint: usize,

    /// Print the discovered vehicle roster and exit without opening a
    /// window.
    #[arg(long)]
    list_cars: bool,

    /// Optional TOML vehicle tuning file. With `--car` it is a full
    /// handling override applied *after* the import (wheel positions and
    /// radii stay pinned to the imported rig; a wheel-count mismatch is
    /// rejected). Without `--car` it configures the synthetic dev car.
    /// A requested file that fails to load or validate is an error, never
    /// a silent fallback.
    #[arg(long)]
    vehicle_config: Option<PathBuf>,

    /// Save a screenshot of the primary window after `--frames` frames and
    /// exit (visual smoke testing). The run waits for the capture to
    /// actually land on disk before reporting `pass`.
    #[arg(long, requires = "frames", conflicts_with = "headless")]
    screenshot: Option<PathBuf>,

    /// Frames to run before taking the screenshot / exiting in smoke mode.
    #[arg(long)]
    frames: Option<u32>,

    /// Start with the free camera active at `x,y,z[,yaw-deg,pitch-deg]`
    /// (screenshot/diagnostic aid).
    #[arg(long, value_name = "x,y,z[,yaw,pitch]", conflicts_with = "headless")]
    cam: Option<String>,

    /// Run without a window or GPU: simulate `--frames` updates
    /// (default 600), print a `smoke=headless-physics` record and exit.
    #[arg(long)]
    headless: bool,
}

/// Where the player vehicle (re)spawns. `trailers` holds each spawned
/// trailer's entity plus its car-space rest offset so a reset can place it
/// back behind the car instead of on top of it.
#[derive(Resource)]
struct SpawnPoint {
    position: Vec3,
    yaw: f32,
    trailers: Vec<(Entity, Vec3)>,
}

/// The imported stock vehicle selected by `--car` or the deterministic
/// stock default (`vpbug`). `None` = synthetic dev car.
#[derive(Resource)]
struct SelectedCar {
    def: Option<VehicleDef>,
    paint: usize,
}

/// The validated vehicle configuration the player car was built from.
#[derive(Resource)]
struct TunedVehicle(VehicleConfig);

/// Marker for the on-screen HUD text.
#[derive(Component)]
struct Hud;

/// Marker for the big error line shown when the world fails to load.
#[derive(Component)]
struct ErrorText;

/// Smoke-test capture: run N frames, take the screenshot (if requested),
/// report a `smoke=visual` record, then exit.
#[derive(Resource)]
struct SmokeTest {
    /// `dev-world` or the city's logical path — the record's `world=`.
    world: String,
    screenshot: Option<PathBuf>,
    frames_left: u32,
    /// Capture whose on-disk arrival we're still waiting for.
    pending: Option<PathBuf>,
    /// Frames left to wait for `pending` before calling it a failure.
    capture_wait: u32,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // `--cam x,y,z[,yaw,pitch]` starts the free camera at a fixed pose
    // (angles in degrees) — a diagnostic/screenshot aid.
    let cam_start = cli.cam.as_deref().map(|s| {
        let parts: Result<Vec<f32>, _> = s.split(',').map(|p| p.trim().parse::<f32>()).collect();
        match parts {
            Ok(f) if f.len() == 3 || f.len() == 5 => Ok(CameraPose {
                position: Vec3::new(f[0], f[1], f[2]),
                yaw: f.get(3).copied().unwrap_or(0.0).to_radians(),
                pitch: f.get(4).copied().unwrap_or(0.0).to_radians(),
            }),
            _ => Err(()),
        }
    });
    let cam_start = match cam_start {
        Some(Ok(c)) => Some(c),
        Some(Err(())) => {
            error!("invalid --cam: expected x,y,z[,yaw,pitch]");
            std::process::exit(2);
        }
        None => None,
    };

    // One mounting policy shared with mm2-inspect: mods > loose install
    // files > archives. The VFS is always built — mods work in the dev
    // world without an MM2 installation.
    let mut vfs = Vfs::new();
    let mut has_mm2 = false;
    if let Some(dir) = &cli.mm2_path {
        match mount_install(&mut vfs, dir, &InstallMount::default()) {
            Ok(report) => {
                info!(
                    archives = report.archives.len(),
                    skipped = report.skipped.len(),
                    loose = report.loose_files,
                    "mounted MM2 installation"
                );
                for (path, err) in &report.skipped {
                    warn!(archive = %path.display(), error = %err, "skipped archive");
                }
                has_mm2 = true;
            }
            Err(e) => {
                error!(path = %dir.display(), error = %e, "failed to mount MM2 installation");
                std::process::exit(2);
            }
        }
    }
    // The app's own synthetic assets (dev-world textures) sit above the
    // install content but below mods, so a mod can replace them.
    let app_assets = PathBuf::from("assets");
    if app_assets.is_dir()
        && let Err(e) = vfs.mount_dir(&app_assets, mm2_assets::priority::OVERRIDE)
    {
        warn!(dir = %app_assets.display(), error = %e, "failed to mount app assets");
    }
    let mut has_mods = false;
    if let Some(mods) = &cli.mods {
        match mount_mods(&mut vfs, mods) {
            Ok(manifests) => {
                has_mods = !manifests.is_empty();
                for m in &manifests {
                    info!(mod_id = %m.id, dir = %mods.display(), "mounted mod");
                }
            }
            Err(e) => warn!(dir = %mods.display(), error = %e, "failed to mount mods"),
        }
    }

    // `--list-cars` needs the VFS only — no window, no GPU.
    if cli.list_cars {
        let catalog = VehicleCatalog::scan(&vfs);
        print_roster(&catalog);
        return;
    }

    // World mode. A specifically requested `--city` always means City —
    // even without an install (a mod may provide it, and a VFS miss is a
    // hard failure rather than a silent dev world). `--dev-world` wins
    // over both.
    let mode = if cli.dev_world {
        if cli.city.is_some() {
            warn!("--city is ignored with --dev-world");
        }
        WorldMode::DevWorld
    } else if cli.city.is_some() || has_mm2 {
        WorldMode::City {
            psdl: format!(
                "city/{}.psdl",
                cli.city.as_deref().unwrap_or("london").to_ascii_lowercase()
            ),
        }
    } else {
        warn!("no --mm2-path and no --dev-world; starting the dev world");
        WorldMode::DevWorld
    };
    let world_label = match &mode {
        WorldMode::DevWorld => "dev-world".to_string(),
        WorldMode::City { psdl } => psdl.clone(),
    };
    let smoke_requested = cli.headless || cli.frames.is_some() || cli.screenshot.is_some();
    let smoke_kind = if cli.headless {
        smoke::KIND_HEADLESS_PHYSICS
    } else {
        smoke::KIND_VISUAL
    };
    let record = |status: smoke::SmokeStatus, detail: String| smoke::SmokeRecord {
        kind: smoke_kind,
        world: world_label.clone(),
        status,
        detail,
    };
    if smoke_requested {
        println!("{}", smoke::header());
    }

    // Vehicle selection: explicit `--car`, else the documented stock
    // default when an installation is mounted, else the synthetic dev car.
    let selected: Option<VehicleDef> = if let Some(query) = &cli.car {
        match mm2_content::load_by_id(&vfs, query, cli.paint) {
            Ok(def) => {
                info!(car = %def.id, name = %def.display_name, paint = cli.paint, "vehicle loaded");
                Some(def)
            }
            Err(e) => {
                error!(car = %query, error = %e, "vehicle failed to load");
                if smoke_requested {
                    println!(
                        "{}",
                        record(smoke::SmokeStatus::Fail, format!("vehicle {query}: {e}")).line()
                    );
                }
                std::process::exit(2);
            }
        }
    } else if has_mm2 {
        match default_stock_car(&vfs, cli.paint) {
            Some(def) => {
                info!(car = %def.id, name = %def.display_name, "default stock vehicle loaded");
                Some(def)
            }
            None => {
                warn!("no usable stock vehicle found; using the synthetic dev car");
                None
            }
        }
    } else {
        if cli.car.is_none() && cli.paint != 0 {
            warn!("--paint has no effect without --car / an MM2 installation");
        }
        None
    };

    // Handling config: `--vehicle-config` is a full override applied after
    // the import when a car was selected; otherwise it tunes the dev car.
    let vehicle = match &cli.vehicle_config {
        Some(path) => match VehicleConfig::load(path) {
            Ok(cfg) => match &selected {
                Some(def) => match mm2_content::assemble::apply_handling_override(&def.config, cfg)
                {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        error!(error = %e, "incompatible --vehicle-config override");
                        std::process::exit(2);
                    }
                },
                None => cfg,
            },
            Err(e) => {
                error!(error = %e, "invalid --vehicle-config");
                std::process::exit(2);
            }
        },
        None => selected
            .as_ref()
            .map(|d| d.config.clone())
            .unwrap_or_default(),
    };
    for w in selected
        .as_ref()
        .map(|d| d.report.warnings.iter().chain(d.model.warnings.iter()))
        .into_iter()
        .flatten()
    {
        warn!(car = ?selected.as_ref().map(|d| d.id.as_str()), "{w}");
    }

    // The session's typed configuration (F01-A): world + mode +
    // difficulty/conditions/densities/seed + vehicle + authority. Only
    // `world`, `vehicle` and the `dev` overrides have runtime consumers
    // today — the rest are the contract F11+ builds against. Developer
    // tweaks stay quarantined in `dev`.
    let session_config = SessionConfig {
        world: mode,
        vehicle: VehicleSelection {
            id: selected.as_ref().map(|d| d.id.clone()),
            paint: cli.paint,
        },
        dev: DevOverrides {
            vehicle_config: cli.vehicle_config.clone(),
            camera: cam_start,
        },
        ..SessionConfig::default()
    };

    // Capability checks with their own status: a requested city with no
    // data source at all is `unavailable` (missing data), and a visual
    // smoke with no display is `unavailable` (no GPU/windowing). Neither
    // is a failure — and neither is allowed to fake a pass.
    if smoke_requested {
        if matches!(&session_config.world, WorldMode::City { .. }) && !has_mm2 && !has_mods {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Unavailable,
                    "requested city needs MM2 data: pass --mm2-path or --mods".into(),
                )
                .line()
            );
            std::process::exit(smoke::SmokeStatus::Unavailable.exit_code());
        }
        if !cli.headless && !display_available() {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Unavailable,
                    "no display detected (DISPLAY/WAYLAND_DISPLAY unset)".into(),
                )
                .line()
            );
            std::process::exit(smoke::SmokeStatus::Unavailable.exit_code());
        }
    }

    // Headless physics smoke: no window, no GPU. Runs and exits here.
    if cli.headless {
        let rec = smoke::headless_smoke(
            &session_config,
            &vfs,
            selected.as_ref(),
            &vehicle,
            cli.frames.unwrap_or(600),
        );
        println!("{}", rec.line());
        std::process::exit(rec.status.exit_code());
    }

    // Menu → Loading: the session resource the app drives through
    // `SessionPhase` transitions (`setup` takes it to Ready → Playing,
    // or Failed). An invalid config is a usage error, not a smoke fail.
    let mut session = Session::new();
    if let Err(e) = session.begin(session_config) {
        error!(error = %e, "invalid session configuration");
        std::process::exit(2);
    }

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "rust-mm2".into(),
                    resolution: (1280, 720).into(),
                    ..default()
                }),
                ..default()
            })
            .disable::<bevy::log::LogPlugin>(),
    )
    .add_plugins(PhysicsPlugins::default())
    .insert_resource(Time::<Fixed>::from_hz(120.0))
    .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
    .insert_resource(ClearColor(Color::srgb(0.5, 0.65, 0.85)))
    .insert_resource(session)
    .insert_resource(SpawnPoint {
        position: Vec3::new(0.0, 1.5, 0.0),
        yaw: 0.0,
        trailers: Vec::new(),
    })
    .insert_resource(Mm2Vfs(vfs))
    .insert_resource(TunedVehicle(vehicle))
    .insert_resource(SelectedCar {
        def: selected,
        paint: cli.paint,
    })
    .init_resource::<car_visual::HeadlightsOn>()
    .insert_resource(if cam_start.is_some() {
        CameraMode::Free
    } else {
        CameraMode::Chase
    })
    .add_plugins(VehiclePlugin)
    .add_systems(Startup, setup)
    .add_systems(FixedUpdate, advance_session_tick)
    .add_systems(
        Update,
        (
            input::vehicle_input.run_if(not(capturing)),
            camera::toggle_camera.run_if(not(capturing)),
            camera::chase_follow,
            camera::free_fly.run_if(not(capturing)),
            reset_input,
            debug_toggle,
            screenshot_input,
            retarget_hud,
            car_visual::update_wheel_visuals,
            car_visual::update_glows,
            car_visual::toggle_headlights,
            car_visual::trailer_input,
            city::animate_textures,
            update_hud,
        ),
    );
    if cli.screenshot.is_some() || cli.frames.is_some() {
        app.insert_resource(SmokeTest {
            world: world_label,
            screenshot: cli.screenshot.clone(),
            frames_left: cli.frames.unwrap_or(600),
            pending: None,
            capture_wait: 0,
        });
        app.add_systems(Update, smoke_test);
    }
    let exit = app.run();
    if let AppExit::Error(code) = exit {
        std::process::exit(code.get() as i32);
    }
}

/// Whether a windowing system is present for a windowed/visual run.
/// macOS and Windows always have one in a GUI session; a headless Linux
/// box does not.
fn display_available() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        true
    }
}

/// Whether a `--frames` capture is running.
///
/// Live input must not reach the camera or the vehicle while one is: the
/// capture opens a window that can take focus from whatever else is on
/// screen, and stray keystrokes fly the free camera away from the `--cam`
/// pose the capture exists to reproduce. Physics keeps running, so the
/// vehicle still settles — it just is not driven.
fn capturing(smoke: Option<Res<SmokeTest>>) -> bool {
    smoke.is_some()
}

/// After N frames, take the screenshot (if requested) and exit.
///
/// A `Failed` world ends the smoke immediately with `status=fail`. A
/// requested screenshot is awaited — `pass` is reported only once the
/// capture file actually exists and is non-empty, never on a fixed delay.
fn smoke_test(
    mut commands: Commands,
    mut st: ResMut<SmokeTest>,
    session: Res<Session>,
    mut exit: MessageWriter<AppExit>,
) {
    let world = st.world.clone();
    let record = |status: smoke::SmokeStatus, detail: String| smoke::SmokeRecord {
        kind: smoke::KIND_VISUAL,
        world: world.clone(),
        status,
        detail,
    };
    if let SessionPhase::Failed(m) = session.phase() {
        println!("{}", record(smoke::SmokeStatus::Fail, m.clone()).line());
        exit.write(AppExit::from_code(
            smoke::SmokeStatus::Fail.exit_code() as u8
        ));
        return;
    }
    if st.frames_left > 0 {
        st.frames_left -= 1;
        return;
    }
    if let Some(path) = st.screenshot.take() {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        st.pending = Some(path);
        // ~15 s at 60 fps — generous for a single frame capture.
        st.capture_wait = 900;
        return;
    }
    if let Some(path) = &st.pending {
        let landed = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
        if landed {
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Pass,
                    format!("frames=done screenshot={} bytes={bytes}", path.display()),
                )
                .line()
            );
            exit.write(AppExit::Success);
        } else if st.capture_wait == 0 {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Fail,
                    format!("screenshot never landed at {}", path.display()),
                )
                .line()
            );
            exit.write(AppExit::from_code(
                smoke::SmokeStatus::Fail.exit_code() as u8
            ));
        }
        st.capture_wait = st.capture_wait.saturating_sub(1);
        return;
    }
    println!(
        "{}",
        record(smoke::SmokeStatus::Pass, "frames=done".into()).line()
    );
    exit.write(AppExit::Success);
}

/// The asset collections world spawning writes into.
#[derive(bevy::ecs::system::SystemParam)]
struct AssetStores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    images: ResMut<'w, Assets<Image>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// Spawn the world, vehicle, cameras, HUD and lights per the session
/// config, driving `Loading → Ready → Playing` (or `Failed`). Everything
/// spawned is stamped with the session's `SessionEntity` generation so a
/// later `Unloading` removes the whole session, not a subset.
#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut assets: AssetStores,
    mut session: ResMut<Session>,
    vfs: Res<Mm2Vfs>,
    vehicle_config: Res<TunedVehicle>,
    selected: Res<SelectedCar>,
    cam_mode: Res<CameraMode>,
    mut spawn: ResMut<SpawnPoint>,
) {
    let owner = SessionEntity(session.generation());
    let Some(config) = session.config().cloned() else {
        error!("setup ran without a session config");
        return;
    };
    let mut world_ok = true;
    match &config.world {
        WorldMode::DevWorld => {
            dev_world::spawn_dev_world(
                &mut commands,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                &vfs.0,
                owner,
            );
            spawn.position = Vec3::new(0.0, 1.5, 0.0);
            spawn.yaw = 0.0;
        }
        WorldMode::City { psdl } => {
            match city::load_city(
                &mut commands,
                &vfs.0,
                psdl,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                owner,
            ) {
                Ok(loaded) => {
                    spawn.position = loaded.spawn;
                    spawn.yaw = loaded.spawn_yaw;
                    info!(report = %loaded.report, "city ready");
                }
                Err(e) => {
                    error!(error = %e, "city failed to load");
                    session
                        .fail(format!("{e}"))
                        .expect("Loading → Failed is a legal transition");
                    world_ok = false;
                }
            }
            // City lighting.
            commands.spawn((
                owner,
                DirectionalLight {
                    illuminance: 15_000.0,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.9, 0.0)),
            ));
            commands.insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.7, 0.75, 0.85),
                brightness: 400.0,
                affects_lightmapped_meshes: false,
            });
        }
    }
    if world_ok {
        session
            .transition(SessionPhase::Ready)
            .expect("Loading → Ready is a legal transition");
    }

    // HUD + error text.
    commands.spawn((
        owner,
        Hud,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.95)),
        // Keeps the line legible over bright facades and sky.
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            ..default()
        },
    ));
    commands.spawn((
        owner,
        ErrorText,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(22.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.4, 0.35)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(120.0),
            left: Val::Px(40.0),
            ..default()
        },
    ));

    // Cameras. The chase boom is sized to the selected vehicle so a city
    // bus and a roadster are both framed sensibly.
    let chase = match &selected.def {
        Some(def) => {
            let [_w, h, d] = def.config.chassis_size;
            ChaseCamera {
                distance: d * 0.85 + 3.5,
                height: h * 0.55 + 1.4,
                look_height: h * 0.45,
                ..default()
            }
        }
        None => ChaseCamera::default(),
    };
    commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: *cam_mode == CameraMode::Chase,
            ..default()
        },
        chase,
        Transform::from_translation(spawn.position + Vec3::new(0.0, 4.0, 9.0)),
    ));
    let (free_xf, free_cam) = match config.dev.camera.as_ref() {
        Some(c) => (
            Transform::from_translation(c.position).with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                c.yaw,
                c.pitch,
                0.0,
            )),
            FreeCamera {
                yaw: c.yaw,
                pitch: c.pitch,
                ..default()
            },
        ),
        None => (
            Transform::from_translation(spawn.position + Vec3::new(0.0, 8.0, 12.0)),
            FreeCamera::default(),
        ),
    };
    commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: *cam_mode == CameraMode::Free,
            ..default()
        },
        free_cam,
        free_xf,
    ));

    // The dynamic player spawns only once the world is `Ready` — after the
    // static colliders above exist, so it can't fall through a half-built
    // city.
    if !world_ok {
        return;
    }
    let vehicle_cfg = &vehicle_config.0;

    // Spawn clearance: keep the collider hull's lowest point off the
    // ground plus a settle margin.
    if let Some(def) = &selected.def {
        let hull_min_y = def
            .config
            .collider_points
            .as_ref()
            .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
            .unwrap_or(-def.config.chassis_size[1] * 0.5);
        spawn.position.y += (0.25 - hull_min_y).max(0.35);
    }
    let vehicle = commands
        .spawn((
            PlayerVehicle,
            owner,
            mm2_vehicle::vehicle_bundle(&vehicle_config.0),
            Transform::from_translation(spawn.position)
                .with_rotation(Quat::from_rotation_y(spawn.yaw)),
            TransformInterpolation,
            // Parents of renderable children need the visibility chain.
            Visibility::Visible,
        ))
        .id();

    match &selected.def {
        // Imported stock vehicle: the model carries the visuals.
        Some(def) => {
            let missing = car_visual::spawn_vehicle_model(
                &mut commands,
                &vfs.0,
                &def.model,
                selected.paint,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                vehicle,
            );
            if !missing.is_empty() {
                warn!(car = %def.id, "missing textures: {}", missing.join(", "));
            }
            if let Some(trailer) = &def.trailer {
                let car_xf = Transform::from_translation(spawn.position)
                    .with_rotation(Quat::from_rotation_y(spawn.yaw));
                let (te, tmissing) = car_visual::spawn_trailer(
                    &mut commands,
                    &vfs.0,
                    trailer,
                    selected.paint,
                    &mut assets.meshes,
                    &mut assets.images,
                    &mut assets.materials,
                    vehicle,
                    car_xf,
                    owner,
                );
                if !tmissing.is_empty() {
                    warn!(car = %def.id, "trailer missing textures: {}", tmissing.join(", "));
                }
                spawn.trailers.push((
                    te,
                    Vec3::from(trailer.car_hitch) - Vec3::from(trailer.trailer_hitch),
                ));
            }
        }
        // Synthetic dev car: cuboid body + cylinder wheels, same
        // mount/spin rig as imported wheels.
        None => {
            let body_mesh = assets
                .meshes
                .add(Cuboid::from_size(Vec3::from(vehicle_cfg.chassis_size)));
            let body_mat = assets.materials.add(StandardMaterial {
                base_color: Color::srgb(0.85, 0.15, 0.1),
                metallic: 0.3,
                perceptual_roughness: 0.5,
                ..default()
            });
            commands
                .entity(vehicle)
                .insert((Mesh3d(body_mesh), MeshMaterial3d(body_mat)));
            let wheel_mesh = assets.meshes.add(Cylinder::new(0.34, 0.25));
            let wheel_mat = assets.materials.add(StandardMaterial {
                base_color: Color::srgb(0.1, 0.1, 0.1),
                perceptual_roughness: 0.9,
                ..default()
            });
            for (i, w) in vehicle_cfg.wheels.iter().enumerate() {
                let mount = commands
                    .spawn((
                        WheelMount { vehicle, index: i },
                        Transform::from_translation(Vec3::from(w.position)),
                    ))
                    .id();
                commands.entity(vehicle).add_child(mount);
                let spin = commands.spawn((WheelSpin, Transform::IDENTITY)).id();
                commands.entity(mount).add_child(spin);
                commands.entity(spin).with_child((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    // Cylinder is Y-aligned: rotate onto the axle (X) and
                    // scale to the configured radius.
                    Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(w.radius / 0.34, 1.0, w.radius / 0.34)),
                ));
            }
        }
    }

    // World built and the player exists — release control.
    session
        .transition(SessionPhase::Playing)
        .expect("Ready → Playing is a legal transition");
}

/// HUD line: speed, gear/direction, RPM, grounded wheels.
fn update_hud(
    session: Res<Session>,
    mut hud: Query<&mut Text, (With<Hud>, Without<ErrorText>)>,
    mut err: Query<&mut Text, (With<ErrorText>, Without<Hud>)>,
    vehicles: Query<(&mm2_vehicle::vehicle::VehicleState, &LinearVelocity), With<PlayerVehicle>>,
    cameras: Query<(&Camera, &Transform)>,
) {
    for mut text in &mut err {
        *text = match session.phase() {
            SessionPhase::Failed(m) => Text::new(format!("world failed to load:\n{m}")),
            _ => Text::new(""),
        };
    }
    let Ok((veh, vel)) = vehicles.single() else {
        for mut text in &mut hud {
            *text = Text::new(match session.phase() {
                SessionPhase::Failed(_) => String::new(),
                SessionPhase::Playing => "no vehicle".to_string(),
                _ => "loading…".to_string(),
            });
        }
        return;
    };
    let speed = vel.0.length() * 3.6;
    let dir = match veh.direction {
        mm2_vehicle::vehicle::DriveDirection::Forward => format!("D{}", veh.gear + 1),
        mm2_vehicle::vehicle::DriveDirection::Reverse => "R".to_string(),
    };
    let grounded = veh.wheels.iter().filter(|w| w.grounded).count();
    let cam = active_cam_pose(&cameras).unwrap_or_default();
    for mut text in &mut hud {
        *text = Text::new(format!(
            "{speed:5.1} km/h  {dir}  {rpm:4.0} rpm  wheels {grounded}/{total}  cam {cam}",
            rpm = veh.rpm,
            total = veh.wheels.len(),
        ));
    }
}

/// The root UI nodes of the HUD.
type HudNodes = Or<(With<Hud>, With<ErrorText>)>;

/// Keep the HUD on whichever camera is active — UI otherwise stays on the
/// first camera and disappears in free-camera mode.
fn retarget_hud(
    mut commands: Commands,
    cameras: Query<(Entity, &Camera)>,
    ui: Query<(Entity, Option<&UiTargetCamera>), HudNodes>,
) {
    let Some((active, _)) = cameras.iter().find(|(_, c)| c.is_active) else {
        return;
    };
    for (node, target) in &ui {
        if target.is_none_or(|t| t.0 != active) {
            commands.entity(node).insert(UiTargetCamera(active));
        }
    }
}

/// The active camera's pose as `x,y,z,yaw,pitch` (angles in degrees) — the
/// exact value `--cam` accepts, so a screenshot's view can be reproduced.
fn active_cam_pose(cameras: &Query<(&Camera, &Transform)>) -> Option<String> {
    let (_, xf) = cameras.iter().find(|(c, _)| c.is_active)?;
    let (yaw, pitch, _) = xf.rotation.to_euler(EulerRot::YXZ);
    let p = xf.translation;
    Some(format!(
        "{:.1},{:.1},{:.1},{:.0},{:.0}",
        p.x,
        p.y,
        p.z,
        // `+ 0.0` turns a rounded −0 into 0.
        yaw.to_degrees().round() + 0.0,
        pitch.to_degrees().round() + 0.0
    ))
}

/// Directory (relative to the working directory, gitignored) that
/// Cmd/Ctrl+P screenshots are saved to.
const SCREENSHOT_DIR: &str = "screenshots";

/// `Cmd+P` (or `Ctrl+P`) saves a screenshot to [`SCREENSHOT_DIR`], named
/// after the time and the camera pose it was taken from.
fn screenshot_input(
    keys: Res<ButtonInput<KeyCode>>,
    cameras: Query<(&Camera, &Transform)>,
    mut commands: Commands,
) {
    let modifier = keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    if !(modifier && keys.just_pressed(KeyCode::KeyP)) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(SCREENSHOT_DIR) {
        error!(dir = SCREENSHOT_DIR, error = %e, "cannot create screenshot directory");
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let cam = active_cam_pose(&cameras).unwrap_or_default();
    let path = PathBuf::from(SCREENSHOT_DIR).join(format!("{secs}_cam_{cam}.png"));
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

/// `R` resets the player vehicle (and any trailer) to the spawn point.
fn reset_input(
    keys: Res<ButtonInput<KeyCode>>,
    spawn: Res<SpawnPoint>,
    player: Query<Entity, With<PlayerVehicle>>,
    mut writer: MessageWriter<ResetVehicle>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    let rot = Quat::from_rotation_y(spawn.yaw);
    writer.write(ResetVehicle {
        entity: player.iter().next(),
        position: spawn.position,
        yaw: spawn.yaw,
    });
    for (entity, offset) in &spawn.trailers {
        writer.write(ResetVehicle {
            entity: Some(*entity),
            position: spawn.position + rot * *offset,
            yaw: spawn.yaw,
        });
    }
}

/// `F1` toggles vehicle physics debug gizmos.
fn debug_toggle(keys: Res<ButtonInput<KeyCode>>, mut dbg: ResMut<VehicleDebugEnabled>) {
    if keys.just_pressed(KeyCode::F1) {
        dbg.0 = !dbg.0;
    }
}

/// Documented stock default when an installation is present and no `--car`
/// was requested: MM2's own menu default, the New Beetle.
const DEFAULT_CAR: &str = "vpbug";

/// Resolve the default vehicle: `vpbug`, falling back to the first
/// loadable expected-stock entry when a partial install lacks it.
fn default_stock_car(vfs: &Vfs, paint: usize) -> Option<VehicleDef> {
    let catalog = VehicleCatalog::scan(vfs);
    let mut candidates = vec![DEFAULT_CAR];
    candidates.extend(mm2_content::EXPECTED_STOCK_ROSTER.iter().copied());
    for id in candidates {
        let Some(entry) = catalog.entries.iter().find(|e| e.id == id) else {
            continue;
        };
        if !entry.is_ready() {
            continue;
        }
        match mm2_content::load_vehicle(vfs, id, paint) {
            Ok(def) => return Some(def),
            Err(e) => warn!(car = %id, error = %e, "stock candidate failed to load"),
        }
    }
    None
}

/// `--list-cars` output: id, name, class, lock status, paints, deps.
fn print_roster(catalog: &VehicleCatalog) {
    if catalog.entries.is_empty() {
        eprintln!(
            "no vehicles discovered — mount an MM2 install with --mm2-path (or mods with --mods)"
        );
        std::process::exit(2);
    }
    println!(
        "{:<14} {:<30} {:<8} {:<5} {:<6} status",
        "id", "name", "class", "lock", "paints"
    );
    for e in &catalog.entries {
        let class = match e.class {
            mm2_content::VehicleClass::Stock => "stock",
            mm2_content::VehicleClass::Mod => "mod",
            mm2_content::VehicleClass::ModOnly => "mod-only",
        };
        let status = match &e.status {
            mm2_content::EntryStatus::Ready => "ready".to_string(),
            mm2_content::EntryStatus::Incomplete { missing } => {
                format!("incomplete: {}", missing.join(", "))
            }
        };
        println!(
            "{:<14} {:<30} {:<8} {:<5} {:<6} {}",
            e.id,
            e.display_name,
            class,
            if e.locked { "yes" } else { "-" },
            e.paints.len(),
            status
        );
    }
    let failures = catalog.stock_audit_failures();
    if !failures.is_empty() {
        eprintln!("\nexpected-stock audit failures:");
        for f in &failures {
            eprintln!("  {f}");
        }
    }
}

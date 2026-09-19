//! `mm2` — executable shell for the MM2-inspired engine.
//!
//! Usage:
//!   cargo run -p mm2_app --bin mm2 -- --dev-world
//!   cargo run -p mm2_app --bin mm2 -- --dev-world --mods examples/mods
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path "/path/to/Midtown Madness 2" [--city london]
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path <dir> --mods <dir> --vehicle-config <toml>

use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use clap::Parser;
use mm2_app::{WorldState, camera, city, dev_world, input, vehicle_visual};
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_game::{ActiveWorld, Mm2Vfs, PlayerVehicle, WorldMode};
use mm2_vehicle::{ResetVehicle, VehicleConfig, VehicleDebugEnabled, VehiclePlugin};
use tracing::{error, info, warn};

use camera::{CameraMode, ChaseCamera, FreeCamera};
use vehicle_visual::WheelVisual;

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

    /// City to load when --mm2-path is given (without --dev-world).
    #[arg(long, default_value = "london")]
    city: String,

    /// Optional TOML vehicle tuning file. Without it the built-in arcade
    /// default is used; a requested file that fails to load or validate is
    /// an error, never a silent fallback.
    #[arg(long)]
    vehicle_config: Option<PathBuf>,

    /// Save a screenshot of the primary window after `--frames` frames and
    /// exit (headless smoke testing).
    #[arg(long, requires = "frames")]
    screenshot: Option<PathBuf>,

    /// Frames to run before taking the screenshot / exiting in smoke mode.
    #[arg(long)]
    frames: Option<u32>,

    /// Start with the free camera active at `x,y,z[,yaw-deg,pitch-deg]`
    /// (screenshot/diagnostic aid).
    #[arg(long, value_name = "x,y,z[,yaw,pitch]")]
    cam: Option<String>,
}

/// Where the player vehicle (re)spawns.
#[derive(Resource)]
struct SpawnPoint {
    position: Vec3,
    yaw: f32,
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

/// Smoke-test capture: take a screenshot after N frames, then exit.
#[derive(Resource)]
struct SmokeTest {
    screenshot: Option<PathBuf>,
    frames_left: u32,
}

/// `--cam` starting pose for the free camera (position + yaw/pitch in
/// radians).
#[derive(Resource)]
struct CamStart {
    position: Vec3,
    yaw: f32,
    pitch: f32,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // Vehicle config: explicit file (must load+validate) or the built-in
    // arcade default.
    let vehicle = match &cli.vehicle_config {
        Some(path) => match VehicleConfig::load(path) {
            Ok(cfg) => cfg,
            Err(e) => {
                error!(error = %e, "invalid --vehicle-config");
                std::process::exit(2);
            }
        },
        None => VehicleConfig::default(),
    };

    // `--cam x,y,z[,yaw,pitch]` starts the free camera at a fixed pose
    // (angles in degrees) — a diagnostic/screenshot aid.
    let cam_start = cli.cam.as_deref().map(|s| {
        let parts: Result<Vec<f32>, _> = s.split(',').map(|p| p.trim().parse::<f32>()).collect();
        match parts {
            Ok(f) if f.len() == 3 || f.len() == 5 => Ok(CamStart {
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
    if let Some(mods) = &cli.mods {
        match mount_mods(&mut vfs, mods) {
            Ok(manifests) => {
                for m in &manifests {
                    info!(mod_id = %m.id, dir = %mods.display(), "mounted mod");
                }
            }
            Err(e) => warn!(dir = %mods.display(), error = %e, "failed to mount mods"),
        }
    }

    let mode = if cli.dev_world || !has_mm2 {
        if !cli.dev_world && !has_mm2 && cli.mm2_path.is_none() {
            warn!("no --mm2-path and no --dev-world; starting the dev world");
        }
        WorldMode::DevWorld
    } else {
        WorldMode::City {
            psdl: format!("city/{}.psdl", cli.city.to_ascii_lowercase()),
        }
    };

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
    .insert_resource(ActiveWorld(mode))
    .insert_resource(SpawnPoint {
        position: Vec3::new(0.0, 1.5, 0.0),
        yaw: 0.0,
    })
    .insert_resource(WorldState::Loading)
    .insert_resource(Mm2Vfs(vfs))
    .insert_resource(TunedVehicle(vehicle))
    .insert_resource(if cam_start.is_some() {
        CameraMode::Free
    } else {
        CameraMode::Chase
    })
    .add_plugins(VehiclePlugin)
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            input::vehicle_input,
            camera::toggle_camera,
            camera::chase_follow,
            camera::free_fly,
            reset_input,
            debug_toggle,
            vehicle_visual::update_wheel_visuals,
            update_hud,
        ),
    );
    if let Some(c) = cam_start {
        app.insert_resource(c);
    }
    if cli.screenshot.is_some() || cli.frames.is_some() {
        app.insert_resource(SmokeTest {
            screenshot: cli.screenshot.clone(),
            frames_left: cli.frames.unwrap_or(600),
        });
        app.add_systems(Update, smoke_test);
    }
    app.run();
}

/// After N frames, take the screenshot (if requested) and exit.
fn smoke_test(mut commands: Commands, mut st: ResMut<SmokeTest>, mut exit: MessageWriter<AppExit>) {
    if st.frames_left > 0 {
        st.frames_left -= 1;
        return;
    }
    if let Some(path) = st.screenshot.take() {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        // Give the capture a couple of frames to complete.
        st.frames_left = 5;
        return;
    }
    exit.write(AppExit::Success);
}

/// The asset collections world spawning writes into.
#[derive(bevy::ecs::system::SystemParam)]
struct AssetStores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    images: ResMut<'w, Assets<Image>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// Spawn the world, vehicle, cameras, HUD and lights per `ActiveWorld`.
#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut assets: AssetStores,
    mode: Res<ActiveWorld>,
    vfs: Res<Mm2Vfs>,
    vehicle_config: Res<TunedVehicle>,
    cam_start: Option<Res<CamStart>>,
    cam_mode: Res<CameraMode>,
    mut spawn: ResMut<SpawnPoint>,
    mut state: ResMut<WorldState>,
) {
    let mut world_ok = true;
    match &mode.0 {
        WorldMode::DevWorld => {
            dev_world::spawn_dev_world(
                &mut commands,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                &vfs.0,
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
            ) {
                Ok(loaded) => {
                    spawn.position = loaded.spawn;
                    spawn.yaw = 0.0;
                    info!(report = %loaded.report, "city ready");
                }
                Err(e) => {
                    error!(error = %e, "city failed to load");
                    *state = WorldState::Failed(format!("{e}"));
                    world_ok = false;
                }
            }
            // City lighting.
            commands.spawn((
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
        *state = WorldState::Ready;
    }

    // HUD + error text.
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
    commands.spawn((
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

    // Cameras.
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: *cam_mode == CameraMode::Chase,
            ..default()
        },
        ChaseCamera::default(),
        Transform::from_translation(spawn.position + Vec3::new(0.0, 4.0, 9.0)),
    ));
    let (free_xf, free_cam) = match cam_start.as_ref() {
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
    let config = &vehicle_config.0;
    let body_mesh = assets
        .meshes
        .add(Cuboid::from_size(Vec3::from(config.chassis_size)));
    let body_mat = assets.materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.15, 0.1),
        metallic: 0.3,
        perceptual_roughness: 0.5,
        ..default()
    });
    let vehicle = commands
        .spawn((
            PlayerVehicle,
            mm2_vehicle::vehicle_bundle(config),
            Mesh3d(body_mesh),
            MeshMaterial3d(body_mat),
            Transform::from_translation(spawn.position)
                .with_rotation(Quat::from_rotation_y(spawn.yaw)),
            TransformInterpolation,
        ))
        .id();

    // Wheel visuals (non-physical; follow suspension state).
    let wheel_mesh = assets.meshes.add(Cylinder::new(0.34, 0.25));
    let wheel_mat = assets.materials.add(StandardMaterial {
        base_color: Color::srgb(0.1, 0.1, 0.1),
        perceptual_roughness: 0.9,
        ..default()
    });
    for i in 0..config.wheels.len() {
        commands.spawn((
            WheelVisual { vehicle, index: i },
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(wheel_mat.clone()),
        ));
    }
}

/// HUD line: speed, gear/direction, RPM, grounded wheels.
fn update_hud(
    state: Res<WorldState>,
    mut hud: Query<&mut Text, (With<Hud>, Without<ErrorText>)>,
    mut err: Query<&mut Text, (With<ErrorText>, Without<Hud>)>,
    vehicles: Query<(&mm2_vehicle::vehicle::VehicleState, &LinearVelocity), With<PlayerVehicle>>,
) {
    for mut text in &mut err {
        *text = match &*state {
            WorldState::Failed(m) => Text::new(format!("world failed to load:\n{m}")),
            _ => Text::new(""),
        };
    }
    let Ok((veh, vel)) = vehicles.single() else {
        for mut text in &mut hud {
            *text = Text::new(match &*state {
                WorldState::Loading => "loading…".to_string(),
                WorldState::Failed(_) => String::new(),
                WorldState::Ready => "no vehicle".to_string(),
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
    for mut text in &mut hud {
        *text = Text::new(format!(
            "{speed:5.1} km/h  {dir}  {rpm:4.0} rpm  wheels {grounded}/{total}",
            rpm = veh.rpm,
            total = veh.wheels.len(),
        ));
    }
}

/// `R` resets the player vehicle to its spawn point.
fn reset_input(
    keys: Res<ButtonInput<KeyCode>>,
    spawn: Res<SpawnPoint>,
    mut writer: MessageWriter<ResetVehicle>,
) {
    if keys.just_pressed(KeyCode::KeyR) {
        writer.write(ResetVehicle {
            entity: None,
            position: spawn.position,
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

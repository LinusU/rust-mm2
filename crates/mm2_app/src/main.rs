//! `mm2` — executable shell for the MM2-inspired engine.
//!
//! Usage:
//!   cargo run -- --dev-world
//!   cargo run -- --mm2-path "/path/to/Midtown Madness 2" [--city london]
//!   cargo run -- --mm2-path <dir> --mods <dir>

mod camera;
mod city;
mod dev_world;
mod input;
mod vehicle_visual;

use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::prelude::*;
use clap::Parser;
use mm2_assets::Vfs;
use mm2_game::{ActiveWorld, Mm2Vfs, PlayerVehicle, WorldMode};
use mm2_vehicle::{ResetVehicle, VehicleConfig, VehicleDebugEnabled, VehiclePlugin};
use tracing::{error, info, warn};

use crate::camera::{CameraMode, ChaseCamera, FreeCamera};
use crate::vehicle_visual::WheelVisual;

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
}

/// Where the player vehicle (re)spawns.
#[derive(Resource)]
struct SpawnPoint {
    position: Vec3,
    yaw: f32,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // Build the VFS: mods > loose install files > archives.
    let mut vfs = Vfs::new();
    let mut has_mm2 = false;
    if let Some(dir) = &cli.mm2_path {
        if let Err(e) = mount_install(&mut vfs, dir) {
            error!(path = %dir.display(), error = %e, "failed to mount MM2 installation");
            std::process::exit(2);
        }
        has_mm2 = true;
    }
    if let Some(mods) = &cli.mods {
        match vfs.mount_mods_dir(mods, 100) {
            Ok(manifests) => info!(mods = manifests.len(), "mods mounted"),
            Err(e) => warn!(dir = %mods.display(), error = %e, "failed to mount mods"),
        }
    }

    let mode = if cli.dev_world || !has_mm2 {
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
    .init_resource::<CameraMode>()
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
        ),
    );
    if has_mm2 {
        app.insert_resource(Mm2Vfs(vfs));
    }
    app.run();
}

/// Mount loose files + all `*.ar` archives of an MM2 installation.
fn mount_install(vfs: &mut Vfs, dir: &std::path::Path) -> Result<(), mm2_assets::AssetsError> {
    vfs.mount_dir(dir, 10)?;
    let mut archives: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| mm2_assets::AssetsError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("ar"))
                .unwrap_or(false)
        })
        .collect();
    archives.sort();
    for ar in &archives {
        // Non-DAVE files (and future archive formats) are skipped, not fatal.
        if let Err(e) = vfs.mount_archive(ar, 0) {
            warn!(archive = %ar.display(), error = %e, "skipped archive");
        }
    }
    Ok(())
}

/// Spawn the world, vehicle, cameras, and lights according to `ActiveWorld`.
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mode: Res<ActiveWorld>,
    vfs: Option<Res<Mm2Vfs>>,
    mut spawn: ResMut<SpawnPoint>,
) {
    match &mode.0 {
        WorldMode::DevWorld => {
            dev_world::spawn_dev_world(&mut commands, &mut meshes, &mut materials);
            spawn.position = Vec3::new(0.0, 1.5, 0.0);
            spawn.yaw = 0.0;
        }
        WorldMode::City { psdl } => {
            if let Some(vfs) = vfs {
                let loaded = city::load_city(
                    &mut commands,
                    &vfs.0,
                    psdl,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                );
                spawn.position = loaded.spawn;
                spawn.yaw = 0.0;
            } else {
                warn!("--mm2-path required for city mode; falling back to dev world");
                dev_world::spawn_dev_world(&mut commands, &mut meshes, &mut materials);
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

    // Player vehicle.
    let config = VehicleConfig::default();
    let body_mesh = meshes.add(Cuboid::from_size(Vec3::from(config.chassis_size)));
    let body_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.15, 0.1),
        metallic: 0.3,
        perceptual_roughness: 0.5,
        ..default()
    });
    let vehicle = commands
        .spawn((
            PlayerVehicle,
            mm2_vehicle::vehicle_bundle(&config),
            Mesh3d(body_mesh),
            MeshMaterial3d(body_mat),
            Transform::from_translation(spawn.position)
                .with_rotation(Quat::from_rotation_y(spawn.yaw)),
            TransformInterpolation,
        ))
        .id();

    // Wheel visuals (non-physical; follow suspension state).
    let wheel_mesh = meshes.add(Cylinder::new(0.34, 0.25));
    let wheel_mat = materials.add(StandardMaterial {
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

    // Cameras.
    commands.spawn((
        Camera3d::default(),
        Camera { ..default() },
        ChaseCamera::default(),
        Transform::from_translation(spawn.position + Vec3::new(0.0, 4.0, 9.0)),
    ));
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            ..default()
        },
        FreeCamera::default(),
        Transform::from_translation(spawn.position + Vec3::new(0.0, 8.0, 12.0)),
    ));
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

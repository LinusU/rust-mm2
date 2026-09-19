//! Synthetic development playground: ground, slopes, bumps, a jump, walls.
//! Requires no MM2 data.
//!
//! The ground plane is textured through the same logical VFS lookup the
//! city importer uses (`texture/dev_road`), so a directory mod containing
//! `texture/dev_road.png` visibly replaces it — and removing the mod
//! restores the shipped base asset.

use avian3d::prelude::*;
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use mm2_assets::Vfs;

use crate::city;

/// Marker for dev-world static geometry.
#[derive(Component)]
pub struct DevWorldEntity;

/// The logical texture stem the dev world ground uses. The shipped base
/// asset is `assets/texture/dev_road.png`; a mod can override it with
/// e.g. `texture/dev_road.png` of its own.
pub const DEV_ROAD_STEM: &str = "dev_road";

/// Tile size for the dev ground texture, metres.
const GROUND_TILE: f32 = 8.0;

fn box_obstacle(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    size: Vec3,
    rotation: Quat,
    color: Color,
) {
    commands.spawn((
        DevWorldEntity,
        Mesh3d(meshes.add(Cuboid::from_size(size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: color,
            ..default()
        })),
        Transform {
            translation: center,
            rotation,
            ..default()
        },
        RigidBody::Static,
        Collider::cuboid(size.x, size.y, size.z),
    ));
}

/// A flat quad mesh (XZ plane, +Y normal) with planar UVs.
fn ground_quad(size: f32, tile: f32) -> Mesh {
    let h = size / 2.0;
    let t = size / tile;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]],
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [t, 0.0], [t, t], [0.0, t]],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    mesh.insert_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]));
    mesh
}

/// Spawn the playground.
pub fn spawn_dev_world(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    vfs: &Vfs,
) {
    let grey = Color::srgb(0.45, 0.47, 0.5);
    let dark = Color::srgb(0.3, 0.3, 0.33);
    let accent = Color::srgb(0.75, 0.35, 0.15);

    // Ground: 400×400 m, 1 m thick, top surface at y=0. The collision box
    // stays untextured; the visible surface is the textured plane on top.
    commands.spawn((
        DevWorldEntity,
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Transform::from_translation(Vec3::new(0.0, -0.5, 0.0)),
    ));
    let ground_mat = match city::load_image(vfs, DEV_ROAD_STEM) {
        Some((image, _has_alpha)) => materials.add(StandardMaterial {
            base_color_texture: Some(images.add(image)),
            perceptual_roughness: 0.95,
            ..default()
        }),
        None => materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.47, 0.5),
            ..default()
        }),
    };
    commands.spawn((
        DevWorldEntity,
        Mesh3d(meshes.add(ground_quad(400.0, GROUND_TILE))),
        MeshMaterial3d(ground_mat),
        Transform::from_translation(Vec3::new(0.0, 0.01, 0.0)),
    ));

    // A gentle slope (10° ramp up toward -Z).
    box_obstacle(
        commands,
        meshes,
        materials,
        Vec3::new(0.0, 2.0, -60.0),
        Vec3::new(30.0, 0.4, 40.0),
        Quat::from_rotation_x(0.17),
        dark,
    );

    // Steeper jump ramp (~20°).
    box_obstacle(
        commands,
        meshes,
        materials,
        Vec3::new(40.0, 3.4, -40.0),
        Vec3::new(10.0, 0.4, 20.0),
        Quat::from_rotation_x(0.36),
        accent,
    );

    // Curb / small step.
    box_obstacle(
        commands,
        meshes,
        materials,
        Vec3::new(-30.0, 0.15, -20.0),
        Vec3::new(20.0, 0.3, 6.0),
        Quat::IDENTITY,
        dark,
    );

    // A row of small bumps.
    for i in 0..5 {
        box_obstacle(
            commands,
            meshes,
            materials,
            Vec3::new(-15.0 + i as f32 * 8.0, 0.1, 30.0),
            Vec3::new(3.0, 0.2 + 0.05 * (i as f32 % 3.0), 3.0),
            Quat::IDENTITY,
            dark,
        );
    }

    // Perimeter walls.
    for (x, z, sx, sz) in [
        (0.0, -200.0, 400.0, 1.0),
        (0.0, 200.0, 400.0, 1.0),
        (-200.0, 0.0, 1.0, 400.0),
        (200.0, 0.0, 1.0, 400.0),
    ] {
        box_obstacle(
            commands,
            meshes,
            materials,
            Vec3::new(x, 2.0, z),
            Vec3::new(sx, 4.0, sz),
            Quat::IDENTITY,
            grey,
        );
    }

    // A few scattered barriers to slalom around.
    for (x, z, yaw) in [
        (10.0, 50.0, 0.3_f32),
        (-20.0, 70.0, -0.5),
        (30.0, 90.0, 0.9),
        (-50.0, -50.0, 0.15),
        (60.0, 20.0, -0.8),
    ] {
        box_obstacle(
            commands,
            meshes,
            materials,
            Vec3::new(x, 0.5, z),
            Vec3::new(4.0, 1.0, 0.4),
            Quat::from_rotation_y(yaw),
            accent,
        );
    }

    // Sun + sky-ish ambient.
    commands.spawn((
        DevWorldEntity,
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

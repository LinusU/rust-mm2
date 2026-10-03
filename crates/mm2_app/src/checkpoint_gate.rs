//! Original, procedural race gantries inspired by the retail silhouette.
//! Dimensions, metalwork and signs are designed artwork, not retail assets.
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
};
use mm2_game::{Checkpoint, RaceDefinition, SessionEntity};

use crate::race::CheckpointMarker;

fn sign_material(
    bytes: &[u8],
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
    .expect("bundled checkpoint sign is a valid PNG");
    let texture = images.add(image);
    materials.add(StandardMaterial {
        base_color_texture: Some(texture.clone()),
        emissive_texture: Some(texture),
        perceptual_roughness: 0.65,
        // Slightly luminous lettering remains readable in overcast/night races.
        emissive: LinearRgba::rgb(0.8, 0.8, 0.8),
        ..default()
    })
}

/// Merge metalwork into one mesh per material instead of one entity per strut.
fn block(mesh: &mut Option<Mesh>, size: Vec3, position: Vec3, rotation: Quat) {
    let part = Mesh::from(Cuboid::from_size(size)).transformed_by(Transform {
        translation: position,
        rotation,
        ..default()
    });
    if let Some(mesh) = mesh {
        mesh.merge(&part).expect("cuboids share vertex attributes");
    } else {
        *mesh = Some(part);
    }
}

fn strut(mesh: &mut Option<Mesh>, a: Vec3, b: Vec3, thickness: f32) {
    let delta = b - a;
    block(
        mesh,
        Vec3::new(thickness, delta.length(), thickness),
        (a + b) * 0.5,
        Quat::from_rotation_arc(Vec3::Y, delta.normalize()),
    );
}

fn panel(mesh: &mut Option<Mesh>, size: Vec2, center: Vec3, aspect: f32) {
    for side in [-1.0, 1.0] {
        let mut rectangle = Mesh::from(Rectangle::from_size(size));
        // Clamp the texture edges into plain blue wings on wide roads rather
        // than stretching the lettering to the trigger's full diameter.
        let u_span = (size.x / size.y / aspect).max(1.0);
        if let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            rectangle.attribute_mut(Mesh::ATTRIBUTE_UV_0)
        {
            for uv in uvs {
                uv[0] = (uv[0] - 0.5) * u_span + 0.5;
            }
        }
        let part = rectangle.transformed_by(
            Transform::from_translation(center + Vec3::Z * side * 0.226).with_rotation(
                Quat::from_rotation_y(if side < 0.0 {
                    std::f32::consts::PI
                } else {
                    0.0
                }),
            ),
        );
        if let Some(mesh) = mesh {
            mesh.merge(&part).expect("signs share vertex attributes");
        } else {
            *mesh = Some(part);
        }
    }
}

fn geometry(cp: &Checkpoint) -> [Option<Mesh>; 4] {
    let mut gold = None;
    let mut frame = None;
    let mut banner = None;
    let mut towers = None;
    // Keep the posts outside the playable trigger diameter. Height clears buses.
    let half_span = cp.radius.max(4.0) + 0.55;
    let height = 7.8;
    let rail = 0.075;
    for x in [-half_span, half_span] {
        block(
            &mut frame,
            Vec3::new(1.35, 0.18, 1.1),
            Vec3::new(x, 0.09, 0.0),
            Quat::IDENTITY,
        );
        for dx in [-0.46, 0.46] {
            for z in [-0.34, 0.34] {
                strut(
                    &mut gold,
                    Vec3::new(x + dx, 0.18, z),
                    Vec3::new(x + dx, height, z),
                    rail,
                );
            }
        }
        for y in [0.25, 1.35, 6.6, height] {
            for z in [-0.34, 0.34] {
                strut(
                    &mut gold,
                    Vec3::new(x - 0.46, y, z),
                    Vec3::new(x + 0.46, y, z),
                    rail,
                );
            }
        }
        // Open X-braced bays above/below the red sign; real depth on both faces.
        for (bottom, top) in [(0.25, 1.35), (6.6, height)] {
            for z in [-0.34, 0.34] {
                for side in [-1.0, 1.0] {
                    strut(
                        &mut gold,
                        Vec3::new(x - side * 0.46, bottom, z),
                        Vec3::new(x + side * 0.46, top, z),
                        0.055,
                    );
                }
            }
        }
        block(
            &mut frame,
            Vec3::new(0.9, 5.15, 0.44),
            Vec3::new(x, 3.975, 0.0),
            Quat::IDENTITY,
        );
        panel(
            &mut towers,
            Vec2::new(0.84, 5.05),
            Vec3::new(x, 3.975, 0.0),
            0.25,
        );
    }
    let width = half_span * 2.0 - 0.92;
    let sign_height = (width / 6.4).clamp(1.35, 2.7);
    let sign_y = height - sign_height * 0.5;
    block(
        &mut frame,
        Vec3::new(width, sign_height, 0.44),
        Vec3::new(0.0, sign_y, 0.0),
        Quat::IDENTITY,
    );
    panel(
        &mut banner,
        Vec2::new(width - 0.12, sign_height - 0.12),
        Vec3::new(0.0, sign_y, 0.0),
        6.4,
    );
    // Narrow gold edging gives the dark sign a clean, visible silhouette.
    for y in [height - sign_height, height] {
        strut(
            &mut gold,
            Vec3::new(-width * 0.5, y, 0.0),
            Vec3::new(width * 0.5, y, 0.0),
            0.045,
        );
    }
    [gold, frame, banner, towers]
}

pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    definition: &RaceDefinition,
    owner: SessionEntity,
) {
    let gold = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.78, 0.16),
        emissive: LinearRgba::rgb(0.08, 0.055, 0.004),
        metallic: 0.65,
        perceptual_roughness: 0.3,
        ..default()
    });
    let frame = materials.add(StandardMaterial {
        base_color: Color::srgb(0.035, 0.045, 0.06),
        metallic: 0.7,
        perceptual_roughness: 0.38,
        ..default()
    });
    let checkpoint = sign_material(
        include_bytes!("../assets/checkpoints/checkpoint.png"),
        images,
        materials,
    );
    let finish = sign_material(
        include_bytes!("../assets/checkpoints/finish.png"),
        images,
        materials,
    );
    let tower = sign_material(
        include_bytes!("../assets/checkpoints/tower.png"),
        images,
        materials,
    );
    for (cp, gate) in definition
        .checkpoints
        .iter()
        .enumerate()
        .map(|(i, cp)| (cp, Some(i)))
        .chain(definition.finish.iter().map(|cp| (cp, None)))
    {
        let root = commands
            .spawn((
                owner,
                CheckpointMarker { gate },
                Transform::from_translation(cp.center)
                    .with_rotation(Quat::from_rotation_y(cp.heading_deg.to_radians())),
                if gate.is_none() {
                    Visibility::Hidden
                } else {
                    Visibility::Visible
                },
            ))
            .id();
        let banner = if gate.is_none() { &finish } else { &checkpoint };
        for (mesh, material) in geometry(cp)
            .into_iter()
            .zip([&gold, &frame, banner, &tower])
        {
            if let Some(mesh) = mesh {
                commands.spawn((
                    owner,
                    ChildOf(root),
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material.clone()),
                    Transform::default(),
                    Visibility::Inherited,
                ));
            }
        }
    }
}

//! A horizontal, bevelled navigation arrow rendered into a transparent HUD
//! image. Its isolated camera never becomes the world listener/PVS camera.
use crate::navarrow::{NavArrow, NavArrowSprites};
use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, ScalingMode, visibility::RenderLayers},
    mesh::PrimitiveTopology,
    prelude::*,
    render::render_resource::TextureFormat,
};
use mm2_game::SessionEntity;

const ARROW_LAYER: usize = 30;
#[derive(Component)]
pub struct NavArrowCamera;
#[derive(Component)]
pub struct NavArrowView;
#[derive(Component, Default)]
pub struct NavArrowModel {
    colors: Option<[Color; 2]>,
}

/// Bevelled extrusion of an arrow in XZ, with forward at -Z.
pub fn arrow_mesh() -> Mesh {
    let outline = [
        Vec2::new(-0.35, 1.0),
        Vec2::new(0.35, 1.0),
        Vec2::new(0.35, 0.0),
        Vec2::new(0.95, 0.0),
        Vec2::new(0.0, -1.2),
        Vec2::new(-0.95, 0.0),
        Vec2::new(-0.35, 0.0),
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut triangle = |a: Vec3, b: Vec3, c: Vec3, shade: f32| {
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for point in [a, b, c] {
            positions.push(point.to_array());
            normals.push(normal.to_array());
            colors.push([shade, shade, shade, 1.0]);
        }
    };
    let vertex =
        |i: usize, y: f32, scale: f32| Vec3::new(outline[i].x * scale, y, outline[i].y * scale);
    for [a, b, c] in [[0, 1, 2], [0, 2, 6], [3, 4, 5]] {
        triangle(
            vertex(a, 0.32, 0.92),
            vertex(b, 0.32, 0.92),
            vertex(c, 0.32, 0.92),
            1.0,
        );
        triangle(
            vertex(c, -0.12, 1.0),
            vertex(b, -0.12, 1.0),
            vertex(a, -0.12, 1.0),
            0.3,
        );
    }
    for i in 0..outline.len() {
        let j = (i + 1) % outline.len();
        let a = vertex(i, 0.22, 1.0);
        let b = vertex(j, 0.22, 1.0);
        let c = vertex(j, 0.32, 0.92);
        let d = vertex(i, 0.32, 0.92);
        triangle(a, b, c, 0.8);
        triangle(a, c, d, 0.8);
        let c = vertex(j, -0.12, 1.0);
        let d = vertex(i, -0.12, 1.0);
        // Different side values make the solid depth readable without depending
        // on the city's lighting, fog or shadows.
        let shade = if outline[j].x > outline[i].x {
            0.55
        } else {
            0.38
        };
        triangle(b, a, d, shade);
        triangle(b, d, c, shade);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

pub fn spawn_nav_arrow_view(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) {
    let target = images.add(Image::new_target_texture(
        384,
        256,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    commands.spawn((
        owner,
        NavArrowCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        RenderLayers::layer(ARROW_LAYER),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 3.6,
                height: 2.4,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 1.6, 7.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.7, 0.12),
        unlit: true,
        ..default()
    });
    commands.spawn((
        owner,
        NavArrowModel::default(),
        Mesh3d(meshes.add(arrow_mesh())),
        MeshMaterial3d(material),
        RenderLayers::layer(ARROW_LAYER),
        Transform::default(),
    ));
    commands.spawn((
        owner,
        NavArrowView,
        ImageNode::new(target),
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(4.0),
            left: Val::Percent(50.0),
            width: Val::Px(180.0),
            height: Val::Px(120.0),
            ..default()
        },
        UiTransform::from_xy(Val::Percent(-50.0), Val::ZERO),
    ));
}

fn paint_color(image: &Image) -> Color {
    let Some(data) = image.data.as_ref() else {
        return Color::WHITE;
    };
    let mut total = [0.0_f32; 3];
    let mut count = 0.0;
    for rgba in data.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 32) {
        for i in 0..3 {
            total[i] += rgba[i] as f32 / 255.0;
        }
        count += 1.0;
    }
    if count == 0.0 {
        return Color::WHITE;
    }
    Color::srgb(total[0] / count, total[1] / count, total[2] / count)
}

/// Logical source state is shared with the authored arrow/HUD contracts. Only
/// the yaw reaches the 3D mesh; camera pitch and checkpoint altitude never do.
pub fn drive_nav_arrow_view(
    source: Query<(&UiTransform, &Visibility, &ImageNode, &NavArrowSprites), With<NavArrow>>,
    mut models: Query<(
        &mut Transform,
        &mut NavArrowModel,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut views: Query<&mut Visibility, (With<NavArrowView>, Without<NavArrow>)>,
    mut cameras: Query<&mut Camera, With<NavArrowCamera>>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let source = source.iter().next();
    let show = source.is_some_and(|(_, vis, _, _)| *vis == Visibility::Visible);
    for mut vis in &mut views {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for mut camera in &mut cameras {
        camera.is_active = show;
    }
    let Some((bearing, _, image, sprites)) = source else {
        return;
    };
    for (mut transform, mut model, material) in &mut models {
        transform.rotation = Quat::from_rotation_y(-bearing.rotation.as_radians());
        if model.colors.is_none()
            && let (Some(ahead), Some(behind)) =
                (images.get(&sprites.ahead), images.get(&sprites.behind))
        {
            model.colors = Some([paint_color(ahead), paint_color(behind)]);
        }
        if let Some(colors) = model.colors
            && let Some(mut material) = materials.get_mut(&material.0)
        {
            material.base_color = colors
                [usize::from(image.image == sprites.behind && sprites.behind != sprites.ahead)];
        }
    }
}

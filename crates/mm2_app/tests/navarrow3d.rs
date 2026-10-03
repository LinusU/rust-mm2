use bevy::{
    asset::RenderAssetUsages,
    ecs::world::CommandQueue,
    mesh::VertexAttributeValues,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use mm2_app::{
    hudmap::WorldCamera3d,
    navarrow::{NavArrow, NavArrowSprites},
    navarrow3d::{self, NavArrowCamera, NavArrowModel, NavArrowView},
};
use mm2_game::SessionEntity;

#[test]
fn arrow_is_a_solid_horizontal_extrusion() {
    let mesh = navarrow3d::arrow_mesh();
    let Some(VertexAttributeValues::Float32x3(vertices)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        panic!("positions")
    };
    let min = vertices.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let max = vertices
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(max - min > 0.4, "the arrow has real thickness");
    assert!(vertices.iter().all(|p| p.iter().all(|v| v.is_finite())));
}

#[test]
fn arrow_turns_on_the_ground_plane_preserves_paints_and_excludes_its_camera() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, navarrow3d::drive_nav_arrow_view);
    let image = |color: &[u8]| {
        Image::new_fill(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            color,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    };
    let ahead = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(image(&[255, 0, 0, 255]));
    let behind = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(image(&[255, 255, 0, 255]));
    let source = app
        .world_mut()
        .spawn((
            NavArrow,
            NavArrowSprites {
                ahead: ahead.clone(),
                behind: behind.clone(),
            },
            ImageNode::new(ahead),
            UiTransform {
                rotation: Rot2::radians(std::f32::consts::FRAC_PI_2),
                ..default()
            },
            Visibility::Visible,
        ))
        .id();
    let mut meshes = Assets::<Mesh>::default();
    let mut images = Assets::<Image>::default();
    let mut materials = Assets::<StandardMaterial>::default();
    let mut queue = CommandQueue::default();
    // Keep source textures in the same store the production renderer reads.
    std::mem::swap(
        &mut images,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    );
    navarrow3d::spawn_nav_arrow_view(
        &mut Commands::new(&mut queue, app.world()),
        &mut meshes,
        &mut images,
        &mut materials,
        SessionEntity(1),
    );
    queue.apply(app.world_mut());
    app.insert_resource(meshes)
        .insert_resource(images)
        .insert_resource(materials);
    app.world_mut().spawn(Camera3d::default());
    app.update();
    let transform = app
        .world_mut()
        .query_filtered::<&Transform, With<NavArrowModel>>()
        .single(app.world())
        .unwrap();
    let forward = transform.rotation * Vec3::NEG_Z;
    assert!(
        forward.distance(Vec3::X) < 0.0001,
        "right bearing points right, never vertically"
    );
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, WorldCamera3d>()
            .iter(app.world())
            .count(),
        1
    );
    assert!(
        app.world_mut()
            .query_filtered::<&Camera, With<NavArrowCamera>>()
            .single(app.world())
            .unwrap()
            .is_active
    );
    app.world_mut()
        .get_mut::<UiTransform>(source)
        .unwrap()
        .rotation = Rot2::radians(std::f32::consts::PI);
    app.world_mut().get_mut::<ImageNode>(source).unwrap().image = behind;
    app.update();
    let transform = app
        .world_mut()
        .query_filtered::<&Transform, With<NavArrowModel>>()
        .single(app.world())
        .unwrap();
    assert!((transform.rotation * Vec3::NEG_Z).distance(Vec3::Z) < 0.0001);
    let handle = app
        .world_mut()
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<NavArrowModel>>()
        .single(app.world())
        .unwrap()
        .0
        .clone();
    assert_eq!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap()
            .base_color,
        Color::srgb(1.0, 1.0, 0.0)
    );
    app.world_mut()
        .get_mut::<Visibility>(source)
        .unwrap()
        .clone_from(&Visibility::Hidden);
    app.update();
    assert!(
        !app.world_mut()
            .query_filtered::<&Camera, With<NavArrowCamera>>()
            .single(app.world())
            .unwrap()
            .is_active
    );
    assert_eq!(
        *app.world_mut()
            .query_filtered::<&Visibility, With<NavArrowView>>()
            .single(app.world())
            .unwrap(),
        Visibility::Hidden
    );
}

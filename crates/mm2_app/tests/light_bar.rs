//! F20-B.1 integration — a cop's light bar through the production
//! model path: the flat `SRNn` quads spawn as hidden `GlowPart`
//! flares, the `SIRENn` boxes stay solid housing, and `update_glows`
//! lights the half the car's `EmergencyLights` flash clock names.
//! Self-authored fixture: a synthetic model, no original data.

use bevy::prelude::*;
use mm2_app::car_visual::{
    self, FLARE_SCALE, FlareSprite, GlowKind, GlowPart, HeadlightsOn, face_flares, update_glows,
};
use mm2_app::settings::GraphicsSettings;
use mm2_assets::Vfs;
use mm2_content::model::{Lod, MeshGroup, ModelPart, PartRole, VehicleModel};
use mm2_formats::pkg::PkgShader;
use mm2_game::{EmergencyLights, LIGHT_BAR_HALF_PERIOD};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehicleState};

fn quad() -> MeshGroup {
    MeshGroup {
        shader_offset: 0,
        positions: vec![[0.0, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0]; 3],
        indices: vec![0, 1, 2],
    }
}

fn part(name: &str, role: PartRole, x: Option<f32>) -> ModelPart {
    ModelPart {
        name: name.into(),
        role,
        lods: vec![(Lod::H, vec![quad()])],
        origin: x.map(|x| [x, 1.5, 0.2]),
        pivot: None,
        recenter: None,
    }
}

/// The retail light-bar shape: a solid body, two `siren` housing boxes
/// (no mtx origin) and four `srn` flares along the bar — even indices
/// on the left (`x < 0`), odd on the right.
fn model() -> VehicleModel {
    VehicleModel {
        parts: vec![
            part("body", PartRole::Body, None),
            part("siren0", PartRole::Siren(0), None),
            part("siren1", PartRole::Siren(1), None),
            part("srn0", PartRole::Siren(0), Some(-0.19)),
            part("srn1", PartRole::Siren(1), Some(0.58)),
            part("srn2", PartRole::Siren(2), Some(-0.56)),
            part("srn3", PartRole::Siren(3), Some(0.19)),
        ],
        paint_jobs: 1,
        shaders_per_paint_job: 1,
        // Retail's `SRNn` shaders: no texture, a solid colour diffuse.
        shaders: vec![PkgShader {
            texture: String::new(),
            diffuse: [0.07, 0.0, 1.0, 1.0],
            ambient: [0.07, 0.0, 1.0, 1.0],
            specular: None,
            emissive: [0.0, 0.0, 0.0, 1.0],
            shininess: 0.25,
        }],
        ..VehicleModel::default()
    }
}

fn app_with_car() -> (App, Entity) {
    app_with_model(model())
}

fn app_with_model(model: VehicleModel) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<HeadlightsOn>()
        .add_systems(Update, update_glows);
    let config = VehicleConfig::default();
    let car = app
        .world_mut()
        .spawn((
            VehicleState::new(&config),
            VehicleInput::default(),
            Transform::default(),
            Visibility::Visible,
        ))
        .id();
    let vfs = Vfs::new();
    let (mut images, mut materials, mut meshes) = {
        let world = app.world_mut();
        (
            std::mem::take(&mut *world.resource_mut::<Assets<Image>>()),
            std::mem::take(&mut *world.resource_mut::<Assets<StandardMaterial>>()),
            std::mem::take(&mut *world.resource_mut::<Assets<Mesh>>()),
        )
    };
    let mut queue = bevy::ecs::world::CommandQueue::default();
    {
        let mut commands = Commands::new(&mut queue, app.world());
        car_visual::spawn_vehicle_model(
            &mut commands,
            &vfs,
            &model,
            0,
            &mut meshes,
            &mut images,
            &mut materials,
            car,
            None,
        );
    }
    queue.apply(app.world_mut());
    let world = app.world_mut();
    *world.resource_mut::<Assets<Image>>() = images;
    *world.resource_mut::<Assets<StandardMaterial>>() = materials;
    *world.resource_mut::<Assets<Mesh>>() = meshes;
    (app, car)
}

/// The flares by bar position (`x` of the node), with their kind and
/// whether they are currently shown, left to right.
fn flares(app: &mut App) -> Vec<(f32, GlowKind, bool)> {
    let mut v: Vec<(f32, GlowKind, bool)> = app
        .world_mut()
        .query::<(&GlowPart, &Transform, &Visibility)>()
        .iter(app.world())
        .map(|(g, t, vis)| (t.translation.x, g.0, *vis == Visibility::Visible))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

fn lit(app: &mut App) -> Vec<f32> {
    flares(app)
        .into_iter()
        .filter(|f| f.2)
        .map(|f| f.0)
        .collect()
}

#[test]
fn only_the_srn_quads_become_flares_and_the_siren_boxes_stay_solid() {
    let (mut app, _car) = app_with_car();
    app.update();
    let f = flares(&mut app);
    assert_eq!(f.len(), 4, "four SRN flares: {f:?}");
    // Left to right: srn2 (-0.56), srn0 (-0.19), srn3 (0.19), srn1 (0.58);
    // the side is the index parity.
    let sides: Vec<GlowKind> = f.iter().map(|f| f.1).collect();
    assert_eq!(
        sides,
        vec![
            GlowKind::Siren(0),
            GlowKind::Siren(0),
            GlowKind::Siren(1),
            GlowKind::Siren(1)
        ]
    );
    // The two housing boxes are ordinary visible nodes, not glows.
    let solid = app
        .world_mut()
        .query_filtered::<&Visibility, (Without<GlowPart>, With<ChildOf>, Without<Mesh3d>)>()
        .iter(app.world())
        .filter(|v| **v == Visibility::Visible)
        .count();
    assert!(solid >= 3, "body + two housing nodes stay visible: {solid}");
}

#[test]
fn a_car_without_emergency_lights_shows_no_flare() {
    let (mut app, _car) = app_with_car();
    app.update();
    assert!(lit(&mut app).is_empty());
}

#[test]
fn the_flash_clock_lights_one_half_then_the_other_then_none_when_removed() {
    let (mut app, car) = app_with_car();
    app.world_mut()
        .entity_mut(car)
        .insert(EmergencyLights::default());
    app.update();
    // Side 0: the left pair.
    assert_eq!(lit(&mut app), vec![-0.56, -0.19]);

    app.world_mut()
        .get_mut::<EmergencyLights>(car)
        .unwrap()
        .advance(LIGHT_BAR_HALF_PERIOD + 0.01);
    app.update();
    assert_eq!(lit(&mut app), vec![0.19, 0.58], "the right pair takes over");

    app.world_mut().entity_mut(car).remove::<EmergencyLights>();
    app.update();
    assert!(lit(&mut app).is_empty(), "signals off, bar dark");
}

/// The reduced-flashing option holds the whole bar lit — nothing
/// alternates — for as long as the signals are on, and the bar still
/// goes dark when they stop. Switching it back restores the flash.
#[test]
fn reduced_flashing_holds_every_flare_lit_while_the_signals_are_on() {
    let (mut app, car) = app_with_car();
    app.insert_resource(GraphicsSettings {
        reduce_flashing: true,
        ..GraphicsSettings::default()
    });
    app.world_mut()
        .entity_mut(car)
        .insert(EmergencyLights::default());
    let all = vec![-0.56, -0.19, 0.19, 0.58];
    for _ in 0..4 {
        app.update();
        assert_eq!(lit(&mut app), all, "both halves lit, whatever the phase");
        app.world_mut()
            .get_mut::<EmergencyLights>(car)
            .unwrap()
            .advance(LIGHT_BAR_HALF_PERIOD + 0.01);
    }

    app.world_mut().entity_mut(car).remove::<EmergencyLights>();
    app.update();
    assert!(lit(&mut app).is_empty(), "steady is not stuck on");

    app.world_mut()
        .resource_mut::<GraphicsSettings>()
        .reduce_flashing = false;
    app.world_mut()
        .entity_mut(car)
        .insert(EmergencyLights::default());
    app.update();
    assert_eq!(lit(&mut app), vec![-0.56, -0.19], "the flash is back");
}

/// Report 7 item 2: the `SRNn` quads must not render as solid coloured
/// boxes. Each flare is a sprite — the authored quad mapped corner to
/// corner (retail's own UVs are uninitialised memory), additive, unlit,
/// two-sided and tinted by the shader's diffuse — grown by
/// `FLARE_SCALE`; the housing boxes stay ordinary opaque geometry.
#[test]
fn the_srn_quads_are_additive_glow_sprites_not_solid_boxes() {
    let (mut app, _car) = app_with_car();
    app.update();
    let world = app.world_mut();
    let nodes: Vec<(Entity, Vec3)> = world
        .query_filtered::<(Entity, &Transform), (With<FlareSprite>, With<GlowPart>)>()
        .iter(world)
        .map(|(e, t)| (e, t.scale))
        .collect();
    assert_eq!(nodes.len(), 4, "one sprite node per SRN flare");
    for (node, scale) in nodes {
        assert_eq!(scale, Vec3::splat(FLARE_SCALE));
        let children = world.get::<Children>(node).expect("the quad child");
        assert_eq!(children.len(), 1);
        let quad = children[0];
        let mat = world.get::<MeshMaterial3d<StandardMaterial>>(quad).unwrap();
        let mat = world
            .resource::<Assets<StandardMaterial>>()
            .get(&mat.0)
            .unwrap();
        assert!(
            matches!(mat.alpha_mode, AlphaMode::Add),
            "added, not opaque"
        );
        assert!(mat.unlit && mat.cull_mode.is_none());
        let c = mat.base_color.to_srgba();
        assert_eq!((c.red, c.green, c.blue), (0.07, 0.0, 1.0), "diffuse tint");
        let mesh = world.get::<Mesh3d>(quad).unwrap();
        let mesh = world.resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
        let uvs: Vec<[f32; 2]> = match mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() {
            bevy::mesh::VertexAttributeValues::Float32x2(v) => v.clone(),
            other => panic!("uv attribute: {other:?}"),
        };
        assert!(
            uvs.iter()
                .all(|uv| uv.iter().all(|c| (0.0..=1.0).contains(c))),
            "the sprite is mapped across the quad: {uvs:?}"
        );
        assert!(
            uvs.windows(2).any(|w| w[0] != w[1]),
            "not one texel: {uvs:?}"
        );
    }
    // The housing boxes stay ordinary geometry: no sprite, no glow.
    let housing = world
        .query_filtered::<&Name, With<FlareSprite>>()
        .iter(world)
        .count();
    assert_eq!(housing, 0);
}

/// A flare turns to the view the player sees through, whatever the
/// car's own orientation, and only its rotation moves.
#[test]
fn a_flare_sprite_faces_the_camera_whatever_the_car_does() {
    let (mut app, car) = app_with_car();
    app.add_systems(Update, face_flares);
    let car_rot = Quat::from_rotation_y(1.2);
    app.world_mut()
        .entity_mut(car)
        .insert(GlobalTransform::from(Transform::from_rotation(car_rot)));
    let view = Quat::from_euler(EulerRot::YXZ, -0.7, -0.4, 0.0);
    app.world_mut().spawn((
        Camera3d::default(),
        GlobalTransform::from(Transform::from_xyz(3.0, 2.0, 9.0).with_rotation(view)),
    ));
    app.update();
    let world = app.world_mut();
    let flares: Vec<Transform> = world
        .query_filtered::<&Transform, With<FlareSprite>>()
        .iter(world)
        .copied()
        .collect();
    assert_eq!(flares.len(), 4);
    for t in flares {
        let facing = car_rot * t.rotation;
        assert!(facing.angle_between(view) < 1e-4, "{facing:?} vs {view:?}");
        assert_eq!(t.scale, Vec3::splat(FLARE_SCALE), "scale untouched");
    }
}

/// The other lamp glows share the defect: their shaders author a black,
/// transparent diffuse and carry the colour in `emissive`, so they tint
/// by the emissive and add — a `HEADLIGHTn` lens is a flare sprite, an
/// `HLIGHT` quad keeps its authored UVs and orientation.
#[test]
fn lamp_shaders_with_a_black_diffuse_glow_by_their_emissive() {
    let lamp = |emissive: [f32; 4]| PkgShader {
        texture: String::new(),
        diffuse: [0.0; 4],
        ambient: [0.0; 4],
        specular: None,
        emissive,
        shininess: 0.0,
    };
    let model = VehicleModel {
        parts: vec![
            part("hlight", PartRole::HeadlightGlow, None),
            part("headlight0", PartRole::Headlight(0), Some(0.6)),
        ],
        paint_jobs: 1,
        shaders_per_paint_job: 1,
        shaders: vec![lamp([1.0, 0.97, 0.68, 1.0])],
        ..VehicleModel::default()
    };
    let (mut app, _car) = app_with_model(model);
    app.update();
    let world = app.world_mut();
    let sprites = world
        .query_filtered::<Entity, (With<FlareSprite>, With<GlowPart>)>()
        .iter(world)
        .count();
    assert_eq!(sprites, 1, "only the HEADLIGHTn lens is a billboard");
    let glows: Vec<Entity> = world
        .query_filtered::<Entity, With<GlowPart>>()
        .iter(world)
        .collect();
    assert_eq!(glows.len(), 2);
    for node in glows {
        let quad = world.get::<Children>(node).unwrap()[0];
        let mat = world.get::<MeshMaterial3d<StandardMaterial>>(quad).unwrap();
        let mat = world
            .resource::<Assets<StandardMaterial>>()
            .get(&mat.0)
            .unwrap();
        assert!(
            matches!(mat.alpha_mode, AlphaMode::Add),
            "{:?}",
            mat.alpha_mode
        );
        assert!(mat.unlit && mat.cull_mode.is_none());
        let c = mat.base_color.to_srgba();
        assert_eq!((c.red, c.green, c.blue), (1.0, 0.97, 0.68), "emissive tint");
    }
}

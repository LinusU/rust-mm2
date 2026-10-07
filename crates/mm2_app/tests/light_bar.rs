//! F20-B.1 integration — a cop's light bar through the production
//! model path: the flat `SRNn` quads spawn as hidden `GlowPart`
//! flares, the `SIRENn` boxes stay solid housing, and `update_glows`
//! lights the half the car's `EmergencyLights` flash clock names.
//! Self-authored fixture: a synthetic model, no original data.

use bevy::prelude::*;
use mm2_app::car_visual::{self, GlowKind, GlowPart, HeadlightsOn, update_glows};
use mm2_app::settings::GraphicsSettings;
use mm2_assets::Vfs;
use mm2_content::model::{Lod, MeshGroup, ModelPart, PartRole, VehicleModel};
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
        ..VehicleModel::default()
    }
}

fn app_with_car() -> (App, Entity) {
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
    let model = model();
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

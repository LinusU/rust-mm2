//! The graphics settings reach the world: the key light's shadows and
//! cascades, and every 3D camera's sample count — the frame after they
//! spawn and whenever the resource changes — while the fill lights never
//! start casting.

use bevy::light::CascadeShadowConfig;
use bevy::prelude::*;
use mm2_app::settings::{
    Antialiasing, GraphicsSettings, GraphicsSettingsPlugin, KeyLight, ShadowQuality,
};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(GraphicsSettingsPlugin);
    app
}

fn spawn_rig(app: &mut App) -> (Entity, Entity, Entity) {
    let key = app
        .world_mut()
        .spawn((
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
            KeyLight,
        ))
        .id();
    let fill = app.world_mut().spawn(DirectionalLight::default()).id();
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    (key, fill, camera)
}

fn cascades(app: &App, light: Entity) -> usize {
    app.world()
        .get::<CascadeShadowConfig>(light)
        .unwrap()
        .bounds
        .len()
}

fn set(app: &mut App, settings: GraphicsSettings) {
    app.insert_resource(settings);
    app.update();
}

#[test]
fn the_defaults_leave_the_shipped_look_untouched() {
    let mut app = app();
    let (key, fill, camera) = spawn_rig(&mut app);
    app.update();
    let world = app.world();
    assert!(
        world
            .get::<DirectionalLight>(key)
            .unwrap()
            .shadow_maps_enabled
    );
    assert!(
        !world
            .get::<DirectionalLight>(fill)
            .unwrap()
            .shadow_maps_enabled
    );
    assert_eq!(
        cascades(&app, key),
        CascadeShadowConfig::default().bounds.len()
    );
    assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Sample4));
}

#[test]
fn shadow_quality_reaches_only_the_key_light() {
    let mut app = app();
    let (key, fill, _) = spawn_rig(&mut app);
    app.update();
    let high = cascades(&app, key);

    set(
        &mut app,
        GraphicsSettings {
            shadows: ShadowQuality::Low,
            ..default()
        },
    );
    assert!(
        app.world()
            .get::<DirectionalLight>(key)
            .unwrap()
            .shadow_maps_enabled
    );
    assert!(cascades(&app, key) < high, "low renders fewer cascades");

    set(
        &mut app,
        GraphicsSettings {
            shadows: ShadowQuality::Off,
            ..default()
        },
    );
    assert!(
        !app.world()
            .get::<DirectionalLight>(key)
            .unwrap()
            .shadow_maps_enabled
    );

    // Turning shadows back on restores the default cascades, and the
    // fill light never casts at any setting.
    set(&mut app, GraphicsSettings::default());
    assert!(
        app.world()
            .get::<DirectionalLight>(key)
            .unwrap()
            .shadow_maps_enabled
    );
    assert_eq!(cascades(&app, key), high);
    assert!(
        !app.world()
            .get::<DirectionalLight>(fill)
            .unwrap()
            .shadow_maps_enabled
    );
}

#[test]
fn antialiasing_reaches_every_camera_including_new_ones() {
    let mut app = app();
    let (_, _, first) = spawn_rig(&mut app);
    app.update();
    set(
        &mut app,
        GraphicsSettings {
            antialiasing: Antialiasing::X2,
            ..default()
        },
    );
    assert_eq!(app.world().get::<Msaa>(first), Some(&Msaa::Sample2));
    // A camera spawned later (a session loading) takes the current count
    // without the setting changing.
    let later = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    assert_eq!(app.world().get::<Msaa>(later), Some(&Msaa::Sample2));
    set(
        &mut app,
        GraphicsSettings {
            antialiasing: Antialiasing::Off,
            ..default()
        },
    );
    assert_eq!(app.world().get::<Msaa>(first), Some(&Msaa::Off));
    assert_eq!(app.world().get::<Msaa>(later), Some(&Msaa::Off));
}

#[test]
fn a_key_light_spawned_after_a_change_is_configured_too() {
    let mut app = app();
    set(
        &mut app,
        GraphicsSettings {
            shadows: ShadowQuality::Off,
            ..default()
        },
    );
    let (key, _, _) = spawn_rig(&mut app);
    app.update();
    assert!(
        !app.world()
            .get::<DirectionalLight>(key)
            .unwrap()
            .shadow_maps_enabled
    );
}

//! F05-B.6 integration — authored engine smoke runs through the
//! production `drive_smoke`/`advance_smoke` systems: the authored
//! pivots and particle spec emit bounded billboard puffs while the
//! vehicle is damaged, remote participants and paused sessions emit
//! nothing, and puffs expire on their authored life.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::damage_fx::{self, SmokeAssets, SmokeFx, SmokeFxReport};
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::VehCarDamage;
use mm2_game::{
    DamageSpec, ImpactId, Player, PlayerControl, Session, SessionConfig, SessionEntity,
    SessionPhase, SmokePolicy, SmokePuff, VehicleDamage, VehicleSmoke,
};

/// A `vehcardamage` fixture shaped like the retail records — two
/// authored pivots, ~1 s puff life, 2×2 atlas frames.
const CARDAMAGE: &str = "type: a\r\n\
vehCarDamage {\r\n\
  MaxDamage 321300.000000\r\n\
  MedDamage 150000.000000\r\n\
  ImpactThreshold 1500.000000\r\n\
  RegenerateRate 0.000000\r\n\
  SmokeOffset 0.100000 0.500000 -1.000000\r\n\
  TextelDamageRadius 0.500000\r\n\
  Position 0.000000 0.000000 0.000000\r\n\
  PositionVar 0.020000 0.020000 0.020000\r\n\
  Velocity 0.000000 0.600000 0.000000\r\n\
  VelocityVar 0.100000 0.100000 0.100000\r\n\
  Life 1.000000\r\n\
  LifeVar 0.200000\r\n\
  Mass 0.000000\r\n\
  MassVar 0.000000\r\n\
  Radius 0.160000\r\n\
  RadiusVar 0.040000\r\n\
  Drag 0.300000\r\n\
  DragVar 0.000000\r\n\
  Damp 0.000000\r\n\
  DampVar 0.000000\r\n\
  DRadius 0.500000\r\n\
  DRadiusVar 0.000000\r\n\
  DAlpha -80.000000\r\n\
  DAlphaVar 0.000000\r\n\
  DRotation 0.000000\r\n\
  DRotationVar 0.000000\r\n\
  InitialBlast 0\r\n\
  SpewRate 0.000000\r\n\
  SpewTimeLimit 0.000000\r\n\
  Gravity 8.700000\r\n\
  TexFrameStart 0\r\n\
  TexFrameEnd 3\r\n\
  BirthFlags 0\r\n\
  Height 0.000000\r\n\
  Intensity 1.000000\r\n\
  Color -167772160\r\n\
  SmokeOffset2 -0.100000 0.500000 -1.000000\r\n\
  DoublePivot 0\r\n\
}\r\n";

const SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
const CAR_POS: Vec3 = Vec3::new(4.0, 0.6, -20.0);
const CAR_YAW: f32 = 0.7;

/// A playing session with the smoke systems and synthetic sprite
/// assets (plain quads + a blended material — UVs are render detail
/// the headless tests don't read). `fx` toggles the `SmokeFx`
/// resource so the absent-assets case is covered.
fn smoke_app(fx: bool) -> App {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(session)
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<SmokeFxReport>()
        .add_systems(
            Update,
            (damage_fx::drive_smoke, damage_fx::advance_smoke).chain(),
        );
    app.finish();
    app.cleanup();

    if fx {
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            });
        let quads = (0..4)
            .map(|_| {
                app.world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(Rectangle::new(1.0, 1.0))
            })
            .collect();
        app.insert_resource(SmokeFx {
            assets: SmokeAssets { quads, material },
        });
    }
    app
}

/// The damaged car — authored damage spec + smoke rig, a local or
/// remote `Player`, and the session stamp like a spawned vehicle.
fn damaged_car(app: &mut App, control: PlayerControl, severity: f32) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let d = VehCarDamage::from_tune(&TuneFile::parse(CARDAMAGE).unwrap()).unwrap();
    let mut damage = VehicleDamage::new(SPEC);
    damage.apply(ImpactId(1), severity);
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id: mm2_game::PlayerId(1),
                control,
            },
            damage,
            VehicleSmoke::new(&d, SmokePolicy::default(), 42),
            Transform::from_translation(CAR_POS).with_rotation(Quat::from_rotation_y(CAR_YAW)),
        ))
        .id()
}

fn puffs(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<SmokePuff>>()
        .iter(app.world())
        .collect()
}

fn report(app: &App) -> &SmokeFxReport {
    app.world().resource::<SmokeFxReport>()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn a_damaged_vehicle_smokes_from_its_authored_pivots() {
    let mut app = smoke_app(true);
    let car = damaged_car(&mut app, PlayerControl::Local, SPEC.med_damage + 10.0);
    run(&mut app, 60);

    let emitted = report(&app).emitted;
    assert!(emitted > 0, "a car past MedDamage emits smoke");
    let puff_list = puffs(&mut app);
    assert!(!puff_list.is_empty());
    // Puffs are born around the authored pivots in car space —
    // ±0.1x/0.5y/−1.0z under the car transform, not at the origin.
    let car_xf = *app.world().get::<Transform>(car).unwrap();
    let near_pivot = puff_list.iter().any(|e| {
        let p = app.world().get::<SmokePuff>(*e).unwrap().position;
        let local = car_xf.to_matrix().inverse().transform_point3(p);
        (local.z - -1.0).abs() < 0.5 && (local.y - 0.5).abs() < 1.0
    });
    assert!(near_pivot, "puffs cluster around the authored pivots");
    // And every puff names the car as its emitter (pool accounting).
    assert!(
        puff_list
            .iter()
            .all(|e| app.world().get::<SmokePuff>(*e).unwrap().emitter == car)
    );
}

#[test]
fn an_intact_vehicle_never_smokes() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.med_damage - 1.0);
    run(&mut app, 120);
    assert_eq!(report(&app).emitted, 0);
    assert!(puffs(&mut app).is_empty());
}

#[test]
fn smoke_stops_once_the_damage_is_repaired() {
    let mut app = smoke_app(true);
    let car = damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 30);
    let emitted = report(&app).emitted;
    assert!(emitted > 0);

    app.world_mut()
        .get_mut::<VehicleDamage>(car)
        .unwrap()
        .repair();
    run(&mut app, 120);
    // No new emissions after repair; the live puffs all expired on
    // their authored life.
    assert_eq!(report(&app).emitted, emitted);
    assert_eq!(report(&app).expired, emitted);
    assert!(puffs(&mut app).is_empty());
}

#[test]
fn puffs_expire_on_their_authored_life_and_stay_bounded() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 600);

    let r = report(&app);
    assert!(r.expired > 0, "puffs die on their authored Life");
    assert!(r.emitted > r.expired, "{r:?}");
    // F05-AC03: the live pool is bounded regardless of run length.
    assert!(
        puffs(&mut app).len() <= SmokePolicy::default().max_live,
        "{} live puffs exceeds the designed bound",
        puffs(&mut app).len()
    );
}

#[test]
fn a_remote_participant_never_smokes_locally() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Remote, SPEC.max_damage);
    run(&mut app, 120);
    // Remote damage is the remote authority's state — its own client
    // renders it; nothing local is emitted.
    assert_eq!(report(&app).emitted, 0);
    assert!(puffs(&mut app).is_empty());
}

#[test]
fn emission_freezes_while_the_session_is_paused() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 30);
    let emitted = report(&app).emitted;
    assert!(emitted > 0);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    run(&mut app, 60);
    // Neither emission nor integration runs paused — live puffs hold.
    assert_eq!(report(&app).emitted, emitted);
    assert_eq!(report(&app).expired, 0);
    assert!(!puffs(&mut app).is_empty());

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 30);
    assert!(report(&app).emitted > emitted, "emission resumes on play");
}

#[test]
fn missing_smoke_assets_emit_nothing() {
    let mut app = smoke_app(false);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 60);
    assert_eq!(report(&app).emitted, 0);
}

#[test]
fn puffs_carry_the_session_stamp_and_bounded_frame() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 30);
    let generation = app.world().resource::<Session>().generation();
    for e in puffs(&mut app) {
        assert_eq!(
            app.world().get::<SessionEntity>(e),
            Some(&SessionEntity(generation)),
            "every puff is session-owned so teardown takes it"
        );
        let puff = app.world().get::<SmokePuff>(e).unwrap();
        assert!((0..4).contains(&puff.frame), "{puff:?}");
    }
}

#[test]
fn billboards_face_the_active_camera() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    let cam_rot = Quat::from_rotation_y(1.1);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        Transform::from_xyz(0.0, 4.0, 8.0).with_rotation(cam_rot),
        GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 8.0).with_rotation(cam_rot)),
    ));
    run(&mut app, 30);
    for e in puffs(&mut app) {
        let rot = app.world().get::<Transform>(e).unwrap().rotation;
        assert!(
            rot.abs_diff_eq(cam_rot, 1e-5),
            "the sprite billboards to the camera: {rot} vs {cam_rot}"
        );
    }
}

#[test]
fn puff_transforms_track_position_radius_and_alpha() {
    let mut app = smoke_app(true);
    damaged_car(&mut app, PlayerControl::Local, SPEC.max_damage);
    run(&mut app, 30);
    for e in puffs(&mut app) {
        let puff = app.world().get::<SmokePuff>(e).unwrap();
        let xf = app.world().get::<Transform>(e).unwrap();
        assert_eq!(xf.translation, puff.position);
        assert!((xf.scale.x - puff.radius * 2.0).abs() < 1e-5);
        // The per-puff material tracks the authored fade.
        let material = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(e)
            .unwrap();
        let alpha = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&material.0)
            .unwrap()
            .base_color
            .alpha();
        assert!((alpha - puff.alpha()).abs() < 1e-4, "{puff:?}");
    }
}

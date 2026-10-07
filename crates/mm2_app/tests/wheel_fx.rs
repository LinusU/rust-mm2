//! F18-B.4 wheel surface particles: the authored `materials.mtl`
//! `ptxindex`/`ptxthreshold` channels select `tune/effects/<name>
//! .asbirthrule` specs (the exe's `ptx_wheel` index table) and draw
//! onto `texture/ptx_wheel` — bound through the real
//! `load_session_world`, gated per grounded wheel by the designed
//! `tire_slippage` quantity (DSN-62), session-stamped and bounded —
//! over a synthetic one-room city, self-authored material table and
//! self-authored rules.
//!
//! The original trigger quantity is unrecovered (UNK-23): these tests
//! pin the designed contract — `ptxthreshold` compares against the
//! wheel's `tire_slippage` utilization with strict `>` — not an
//! original-parity claim.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::session::{self, SessionControl};
use mm2_app::wheel_fx::{WheelFx, WheelFxReport};
use mm2_app::{camera, contracts, wheel_fx};
use mm2_assets::Vfs;
use mm2_formats::materials::PtxChannels;
use mm2_game::{
    ImpactEvent, Mm2Vfs, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerId, RaceStarted,
    ResultLedger, Session, SessionEntity, SessionPhase, SurfaceMaterial, TimeOfDay, Weather,
    WheelPtx, WheelPuff, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig, VehiclePlugin, VehicleState};

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// The authored `ptxindex`/`ptxthreshold` pair — `_default` smokes
/// (4/-1), `testgrass` dusts and grasses (1/2 at retail's 0.25/0.5),
/// `testwater` splashes on any tire work (-1/6 at retail's 0/0) and
/// `testquiet` is the authored-dark pair (-1/-1).
const MATERIALS_MTL: &str = "mtl _default {\n\
  elasticity: 0.9\n\
  friction: 1.0\n\
  effect: none\n\
  sound: 0\n\
  drag: 0.0\n\
  width: 0.0\n\
  height: 0.0\n\
  depth: 0.0\n\
  ptxindex: 4 -1\n\
  ptxthreshold: 0.25 0.5\n\
}\n\
mtl testgrass {\n\
  elasticity: 0.0\n\
  friction: 1.0\n\
  effect: none\n\
  sound: 1\n\
  drag: 0.0\n\
  width: 0.0\n\
  height: 0.0\n\
  depth: 0.0\n\
  ptxindex: 1 2\n\
  ptxthreshold: 0.25 0.5\n\
}\n\
mtl testwater {\n\
  elasticity: 0.0\n\
  friction: 0.5\n\
  effect: none\n\
  sound: 2\n\
  drag: 0.1\n\
  width: 0.0\n\
  height: 0.0\n\
  depth: 0.0\n\
  ptxindex: -1 6\n\
  ptxthreshold: 0.0 0.0\n\
}\n\
mtl testquiet {\n\
  elasticity: 0.0\n\
  friction: 1.0\n\
  effect: none\n\
  sound: 3\n\
  drag: 0.0\n\
  width: 0.0\n\
  height: 0.0\n\
  depth: 0.0\n\
  ptxindex: -1 -1\n\
  ptxthreshold: 0.25 0.5\n\
}\n";

const MATERIALS_CSV: &str = "texture,physics\ntest_road,testgrass\n";

/// One self-authored standalone rule — `frame0`/`frame1` shift each
/// effect onto a distinct `ptx_wheel` tile range so the emitted puffs
/// name which channel produced them, and `radius` scales per rule for
/// the same reason. `blast`/`spew` keep the retail burst+held shape.
fn rule(frame0: i64, frame1: i64, radius: f32, blast: i64, spew: f32) -> String {
    format!(
        "type: a\nasBirthRule {{\n\
         \x20 PositionVar 0.200000 0.000000 0.200000\n\
         \x20 Velocity 0.000000 1.000000 0.000000\n\
         \x20 VelocityVar 0.500000 0.500000 0.500000\n\
         \x20 Life 0.500000\n\
         \x20 LifeVar 0.000000\n\
         \x20 Mass 0.300000\n\
         \x20 MassVar 0.000000\n\
         \x20 Radius {radius:.6}\n\
         \x20 RadiusVar 0.000000\n\
         \x20 Drag 0.000000\n\
         \x20 DragVar 0.000000\n\
         \x20 Damp 0.000000\n\
         \x20 DampVar 0.000000\n\
         \x20 DRadius 0.100000\n\
         \x20 DRadiusVar 0.000000\n\
         \x20 DAlpha -64.000000\n\
         \x20 DAlphaVar 0.000000\n\
         \x20 DRotation 0.000000\n\
         \x20 DRotationVar 0.000000\n\
         \x20 InitialBlast {blast}\n\
         \x20 SpewRate {spew:.6}\n\
         \x20 SpewTimeLimit 0.000000\n\
         \x20 Gravity -1.000000\n\
         \x20 TexFrameStart {frame0}\n\
         \x20 TexFrameEnd {frame1}\n\
         \x20 BirthFlags 0\n\
         \x20 Height 0.000000\n\
         \x20 Intensity 1.000000\n\
         \x20 Color -1\n\
         }}\n"
    )
}

/// The eight `PTX_RULE_NAMES` effects — distinct tile ranges so a
/// puff's `frame_start` names its rule: dirt 0, dust 8, grass 16,
/// leaf 24, smoke 32, snow 40, splash 48, rock 56.
const RULE_INDEX: [(&str, i64, f32, i64, f32); 8] = [
    ("dirt", 0, 0.3, 4, 10.0),
    ("dust", 8, 0.5, 4, 30.0),
    ("grass", 16, 2.0, 4, 30.0),
    ("leaf", 24, 0.4, 4, 10.0),
    ("smoke", 32, 1.0, 4, 20.0),
    ("snow", 40, 0.3, 4, 10.0),
    ("splash", 48, 0.6, 8, 30.0),
    ("rock", 56, 0.2, 4, 10.0),
];

/// The full authored set — every rule plus the `ptx_wheel` atlas
/// stand-in (a real PNG through the same decode path; the retail
/// `.tex` stays out of git).
fn write_wheel_fx(d: &Path) {
    for (name, f0, radius, blast, spew) in RULE_INDEX {
        write(
            d,
            &format!("tune/effects/{name}.asbirthrule"),
            rule(f0, f0 + 7, radius, blast, spew),
        );
    }
    write(d, "city/materials.mtl", MATERIALS_MTL);
    write(d, "city/materials.csv", MATERIALS_CSV);
    std::fs::write(
        d.join("texture/ptx_wheel.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
}

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// The same one-room synthetic PSDL `environment`/`precip` stamp — a
/// road (x −5..5, z 0..20) plus a ground fan — so this test stays
/// self-contained (each `tests/` file is a crate).
fn city_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[
        [-5., 0., 0.],
        [-3., 0., 0.],
        [3., 0., 0.],
        [5., 0., 0.], // road section 0: sw_l, rl, rr, sw_r
        [-5., 0., 20.],
        [-3., 0., 20.],
        [3., 0., 20.],
        [5., 0., 20.], // road section 1
        [10., 0., 0.],
        [10., 0., 10.],
        [20., 0., 10.],
        [20., 0., 0.], // fan (clockwise in x,z)
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut attr_words: Vec<u16> = Vec::new();
    let attr = |words: &mut Vec<u16>, word: u16, data: &[u16]| {
        words.push(word);
        words.extend_from_slice(data);
    };
    attr(&mut attr_words, 0x0a << 3, &[1]); // texture ref → textures[0]
    attr(&mut attr_words, 0x00, &[2, 0, 1, 2, 3, 4, 5, 6, 7]); // counted road
    attr(&mut attr_words, 0x06 << 3, &[2, 8, 9, 10, 11]); // counted fan
    attr(&mut attr_words, (0x0b << 3) | 6, &[1, 2, 3, 2, 8, 9]); // facade
    attr(&mut attr_words, (0x07 << 3) | 4, &[0, 2, 8, 9]); // facade bound
    attr(&mut attr_words, (0x03 << 3) | 4, &[2, 0, 8, 9]); // sliver
    attr(&mut attr_words, (0x09 << 3) | 3 | 0x80, &[9, 9, 9]); // tunnel (last)
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // neighbour room
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[-5., 0., 0.]); // bounds min
    push_f32s(&mut d, &[45., 6., 20.]); // bounds max
    push_f32s(&mut d, &[20., 3., 10.]); // bounds centre
    push_f32s(&mut d, &[30.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// A minimal city install: `city/test.psdl` + its road texture, the
/// surface table pair and whatever `tune/effects/*`/`texture/ptx_*`
/// the caller writes.
fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn city_config(weather: u8) -> mm2_game::SessionConfig {
    mm2_game::SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        conditions: mm2_game::SessionConditions {
            time_of_day: TimeOfDay::new(1).unwrap(),
            weather: Weather::new(weather).unwrap(),
        },
        ..mm2_game::SessionConfig::default()
    }
}

/// The same session wiring `environment.rs`/`precip.rs` run — the
/// real `load_session_world` (which binds `SurfaceTables`, `WheelFx`
/// and the report) plus the wheel-fx emit/advance pair and unload
/// reset on a minimal headless app.
fn city_app(vfs: Vfs) -> App {
    city_app_in(vfs, 0)
}

/// `city_app` under one weather selector (`3` is `rainy`).
fn city_app_in(vfs: Vfs, weather: u8) -> App {
    let mut session = Session::new();
    session.begin(city_config(weather)).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::precip::PrecipReport>()
        .init_resource::<WheelFxReport>()
        .init_resource::<mm2_app::audio::AudioReport>()
        .init_resource::<Assets<mm2_app::audio::PcmAudio>>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (wheel_fx::emit_wheel_fx, wheel_fx::advance_wheel_fx).chain(),
                mm2_app::audio::surface_voices,
                wheel_fx::reset_wheel_fx_report.run_if(session::unloading),
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

fn playing(app: &mut App) -> bool {
    matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    )
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

/// The authored index of a named material in the session's
/// `SurfaceTables`.
fn material_index(app: &mut App, name: &str) -> u16 {
    let tables = app.world().resource::<mm2_content::SurfaceTables>();
    tables
        .set
        .index(name)
        .and_then(|i| u16::try_from(i).ok())
        .unwrap_or_else(|| panic!("{name} not in the surface set"))
}

/// A real city collider carrying `material` — the entity a wheel
/// raycast's `contact_entity` reports.
fn city_collider(app: &mut App, material: u16) -> Entity {
    app.world_mut()
        .query::<(Entity, &SurfaceMaterial)>()
        .iter(app.world())
        .find(|(_, m)| **m == SurfaceMaterial::Authored(material))
        .map(|(e, _)| e)
        .expect("a city collider with the named material")
}

/// A bare entity carrying just the `SurfaceMaterial` classification —
/// the shape `contact_entity` resolution reads, for materials the
/// city's texture table does not reach.
fn surface_tag(app: &mut App, material: u16) -> Entity {
    app.world_mut()
        .spawn(SurfaceMaterial::Authored(material))
        .id()
}

/// A drivable car like `load_session_world`/`spawn_opponents` stamps,
/// minus the physics bundle — the wheel-fx systems read `VehicleState`
/// directly, so wheel contact fields are written like the sim writes
/// them (the audio surface-voice tests shape contacts the same way).
/// `slot` pins the object id so emission reseeds identically.
fn test_car(app: &mut App, slot: u32) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let config = VehicleConfig::default();
    app.world_mut()
        .spawn((
            Vehicle {
                config: config.clone(),
            },
            VehicleState::new(&config),
            ObjectIdentity(ObjectId { generation, slot }),
            SessionEntity(generation),
            Transform::default(),
        ))
        .id()
}

/// Write one wheel's contact fields — `contact` is the
/// `SurfaceMaterial`-carrying entity the wheel rests on (or `None`
/// airborne), `demand` the slippage to read: how far past its
/// longitudinal limit the tire has slid, written as the traction
/// demand that produces it (0 is a gripping tire).
fn set_wheel(app: &mut App, car: Entity, wheel: usize, contact: Option<Entity>, demand: f32) {
    let config = &app.world().get::<Vehicle>(car).unwrap().config;
    let ratio = config.wheels[wheel]
        .tires
        .as_ref()
        .unwrap_or(&config.tires)
        .peak_slip_ratio;
    let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
    let w = &mut state.wheels[wheel];
    w.grounded = contact.is_some();
    w.contact_entity = contact;
    w.contact_point = Vec3::new(0.0, 0.0, 5.0);
    w.contact_normal = Vec3::Y;
    w.traction_demand = if demand > 0.0 {
        1.0 + demand * ratio
    } else {
        0.0
    };
}

/// Live puffs `car` owns.
fn car_puffs(app: &mut App, car: Entity) -> Vec<(Vec3, i64, f32)> {
    app.world_mut()
        .query::<&WheelPuff>()
        .iter(app.world())
        .filter(|p| p.emitter == car)
        .map(|p| (p.position, p.frame_start, p.radius))
        .collect()
}

/// A session over the full authored set binds all eight rules and the
/// atlas: the report counts the bind, `WheelFx` carries a spec per
/// `PTX_RULE_NAMES` slot and the authored fields survive the
/// standalone-record conversion (the effects-superset fields too).
#[test]
fn a_session_binds_the_eight_effect_rules() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<WheelFxReport>();
    assert_eq!(report.loaded, 8);
    assert_eq!(report.failed, 0);
    assert!(report.texture, "the ptx_wheel stand-in resolved");
    let fx = app.world().resource::<WheelFx>();
    for (i, (name, f0, _, _, _)) in RULE_INDEX.iter().enumerate() {
        let spec = fx.specs[i].as_ref().unwrap_or_else(|| panic!("{name}"));
        assert_eq!(spec.tex_frame_start, *f0, "{name} frame range");
    }
    // The effects-superset fields ride through `From<&StandaloneBirthRule>`.
    assert_eq!(fx.specs[4].as_ref().unwrap().color, -1);
    assert_eq!(fx.specs[4].as_ref().unwrap().intensity, 1.0);
}

/// A rule the VFS cannot deliver counts `failed` once and its slot
/// stays dark — never substituted (F18-AC06): a slipping wheel on a
/// splash-channeled surface emits nothing.
#[test]
fn a_missing_rule_counts_failed_and_stays_dark() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    std::fs::remove_file(tmp.path().join("tune/effects/splash.asbirthrule")).unwrap();
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<WheelFxReport>();
    assert_eq!(report.loaded, 7);
    assert_eq!(report.failed, 1, "splash counted once at bind");
    assert!(app.world().resource::<WheelFx>().specs[6].is_none());

    let water = material_index(&mut app, "testwater");
    let surface = surface_tag(&mut app, water);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 30);
    assert_eq!(
        app.world().resource::<WheelFxReport>().emitted,
        0,
        "the missing rule's slot emitted nothing"
    );
}

/// An unresolved `ptx_wheel` still binds the table — the report flags
/// the texture miss and puffs emit untextured rather than sinking the
/// session.
#[test]
fn a_missing_atlas_flags_the_report_and_still_emits() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    std::fs::remove_file(tmp.path().join("texture/ptx_wheel.png")).unwrap();
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));
    assert!(!app.world().resource::<WheelFxReport>().texture);
    assert_eq!(app.world().resource::<WheelFxReport>().loaded, 8);

    let grass = material_index(&mut app, "testgrass");
    let surface = surface_tag(&mut app, grass);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 30);
    assert!(
        app.world().resource::<WheelFxReport>().emitted > 0,
        "untextured puffs still emit"
    );
}

/// The production leg: a slipping wheel grounded on the city's real
/// `testgrass` collider (texture → csv → mtl → `SurfaceMaterial`)
/// emits session-stamped puffs at its contact point — `testgrass`
/// authors dust (1) and grass (2) at 0.25/0.5, and demand 0.8 opens
/// both channels.
#[test]
fn a_slipping_wheel_emits_both_authored_channels_at_its_contact() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let grass = material_index(&mut app, "testgrass");
    let collider = city_collider(&mut app, grass);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(collider), 0.8);
    run(&mut app, 30);

    let puffs = car_puffs(&mut app, car);
    assert!(!puffs.is_empty(), "a slipping wheel emits");
    let generation = app.world().resource::<Session>().generation();
    let contact = Vec3::new(0.0, 0.0, 5.0);
    let stamped = app
        .world_mut()
        .query::<(&WheelPuff, &SessionEntity)>()
        .iter(app.world())
        .filter(|(p, _)| p.emitter == car)
        .all(|(_, s)| s.0 == generation);
    assert!(stamped, "puffs carry the session stamp");
    let dust = puffs.iter().filter(|(_, f, _)| *f == 8).count();
    let grass_p = puffs.iter().filter(|(_, f, _)| *f == 16).count();
    assert!(dust > 0 && grass_p > 0, "both authored channels emit");
    let near = puffs
        .iter()
        .all(|(p, _, _)| (p.x - contact.x).abs() < 1.0 && (p.z - contact.z).abs() < 1.0);
    assert!(near, "puffs spawn at the wheel's contact point");
    assert!(app.world().get::<WheelPtx>(car).is_some(), "the rig bound");
}

/// `ptxthreshold` gates per channel (DSN-62): demand 0.3 opens the
/// 0.25 channel (dust, tiles 8..=15) but not the 0.5 one (grass,
/// tiles 16..=23).
#[test]
fn the_authored_threshold_gates_each_channel() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let grass = material_index(&mut app, "testgrass");
    let surface = surface_tag(&mut app, grass);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.3);
    run(&mut app, 30);

    let puffs = car_puffs(&mut app, car);
    assert!(!puffs.is_empty(), "the 0.25 channel opened");
    assert!(
        puffs.iter().all(|(_, f, _)| *f == 8),
        "only dust emitted — the 0.5 grass channel stayed dark: {puffs:?}"
    );
}

/// A parked wheel reads `q = 0` and stays dark even on the
/// threshold-0 water channel — strict `>` is what keeps standing
/// water contact from sprinkling a stationary car (the designed
/// reading, UNK-23).
#[test]
fn a_parked_wheel_stays_dark_even_on_water() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let water = material_index(&mut app, "testwater");
    let surface = surface_tag(&mut app, water);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.0);
    run(&mut app, 30);
    assert_eq!(app.world().resource::<WheelFxReport>().emitted, 0);
}

/// An airborne wheel closes its gates — and regrounding re-fires the
/// authored `InitialBlast` even on the same surface.
#[test]
fn a_reground_re_fires_the_initial_blast() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let water = material_index(&mut app, "testwater");
    let surface = surface_tag(&mut app, water);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 10);
    let first = app.world().resource::<WheelFxReport>().emitted;
    assert!(first > 0);

    // Airborne — nothing emits, the gate releases.
    set_wheel(&mut app, car, 0, None, 0.0);
    run(&mut app, 60); // let live splash puffs (life 0.5 s) expire
    let live = car_puffs(&mut app, car);
    assert!(live.is_empty(), "old puffs expired: {live:?}");

    // Reground on the same surface — the edge re-fires `InitialBlast 8`
    // (one frame, so the 30/s spew carry has not floored to a puff).
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 1);
    let new: Vec<_> = car_puffs(&mut app, car);
    assert_eq!(
        new.len(),
        8,
        "regrounding re-fires the authored blast: {new:?}"
    );
    assert!(new.iter().all(|(_, f, _)| *f == 48), "splash tiles");
}

/// An unmarked contact resolves the `_default` block's channels
/// (smoke/-1) — the same inheritance the surface-sound lookup makes.
#[test]
fn an_unmarked_contact_reads_the_default_material() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    // No `SurfaceMaterial` component — the same `Unspecified` reading
    // an untagged collider gives.
    let bare = app.world_mut().spawn_empty().id();
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(bare), 0.8);
    run(&mut app, 30);

    let puffs = car_puffs(&mut app, car);
    assert!(!puffs.is_empty(), "_default's smoke channel emits");
    assert!(
        puffs.iter().all(|(_, f, _)| *f == 32),
        "only the authored 4 slot emitted: {puffs:?}"
    );
}

/// The authored `-1 -1` pair is the dark surface — a slipping wheel
/// on `testquiet` emits nothing, and `ptxindex` values outside the
/// eight-name table do the same.
#[test]
fn a_dark_surface_pair_emits_nothing() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let quiet = material_index(&mut app, "testquiet");
    let surface = surface_tag(&mut app, quiet);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 30);
    assert_eq!(app.world().resource::<WheelFxReport>().emitted, 0);
}

/// Remote participants never emit — their own client renders them
/// (the same skip every effect system makes). A car with no
/// `ObjectIdentity` stays dark too: there is no session-stable seed
/// to replicate the stream from.
#[test]
fn remote_or_unidentified_cars_stay_dark() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let grass = material_index(&mut app, "testgrass");
    let surface = surface_tag(&mut app, grass);

    let remote = test_car(&mut app, 78);
    app.world_mut().entity_mut(remote).insert(Player {
        id: PlayerId(7),
        control: PlayerControl::Remote,
    });
    set_wheel(&mut app, remote, 0, Some(surface), 0.8);

    let config = VehicleConfig::default();
    let anonymous = app
        .world_mut()
        .spawn((
            Vehicle {
                config: config.clone(),
            },
            VehicleState::new(&config),
        ))
        .id();
    set_wheel(&mut app, anonymous, 0, Some(surface), 0.8);

    run(&mut app, 30);
    assert_eq!(
        app.world().resource::<WheelFxReport>().emitted,
        0,
        "remote and unidentifiable cars emitted nothing"
    );
    assert!(app.world().get::<WheelPtx>(anonymous).is_none());
    assert!(app.world().get::<WheelPtx>(remote).is_none());
}

/// The seeded stream replays: two identical sessions emit the same
/// puffs at the same positions for the same object id (F18 req 5's
/// deterministic leg).
#[test]
fn emission_replays_identically_for_the_same_object_id() {
    let run_once = || {
        let tmp = city_install();
        write_wheel_fx(tmp.path());
        let mut app = city_app(vfs_of(tmp.path()));
        app.update();
        assert!(playing(&mut app));
        let grass = material_index(&mut app, "testgrass");
        let surface = surface_tag(&mut app, grass);
        let car = test_car(&mut app, 77);
        set_wheel(&mut app, car, 0, Some(surface), 0.8);
        run(&mut app, 30);
        let mut puffs = car_puffs(&mut app, car);
        puffs.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        (puffs, app.world().resource::<WheelFxReport>().emitted)
    };
    let (a_puffs, a_emitted) = run_once();
    let (b_puffs, b_emitted) = run_once();
    assert_eq!(a_emitted, b_emitted);
    assert_eq!(a_puffs, b_puffs, "same seed → same puff stream");
}

/// The pool is bounded per vehicle and puffs expire on `Life` —
/// emitted = live + expired + dropped, no leak ledger.
#[test]
fn the_pool_stays_bounded_and_puffs_expire() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let grass = material_index(&mut app, "testgrass");
    let surface = surface_tag(&mut app, grass);
    let car = test_car(&mut app, 77);
    for w in 0..4 {
        set_wheel(&mut app, car, w, Some(surface), 0.9);
    }
    run(&mut app, 90);

    let car_live = car_puffs(&mut app, car).len() as u64;
    assert!(
        car_live <= app.world().get::<WheelPtx>(car).unwrap().policy.max_live as u64,
        "live {car_live} exceeds the per-vehicle bound"
    );
    // Conservation is global — the session's real player car can emit
    // a landing-transient puff too, so `live` counts every emitter.
    let live = app
        .world_mut()
        .query::<&WheelPuff>()
        .iter(app.world())
        .count() as u64;
    let report = app.world().resource::<WheelFxReport>();
    assert!(report.emitted > 0);
    assert!(report.expired > 0, "0.5 s puffs expired across 90 frames");
    assert_eq!(
        report.emitted,
        report.expired + live,
        "every emitted puff is live or expired"
    );
    // `dropped` counts the bound's discards — wants that never spawned.
    assert!(report.dropped > 0, "the bound discarded overflow");
}

/// Teardown sweeps the puffs with the session and removes `WheelFx`;
/// the restart rebinds the table and the report recounts from zero.
#[test]
fn restart_rebinds_the_table_and_sweeps_puffs() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let grass = material_index(&mut app, "testgrass");
    let surface = surface_tag(&mut app, grass);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 20);
    assert!(app.world().resource::<WheelFxReport>().emitted > 0);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    for _ in 0..60 {
        app.update();
        if playing(&mut app) {
            break;
        }
    }
    assert!(playing(&mut app), "restart never returned to Playing");
    assert_eq!(app.world().resource::<Session>().generation(), 2);

    // Fresh bind, fresh counters — no gen-1 puff survives.
    assert_eq!(app.world().resource::<WheelFxReport>().loaded, 8);
    let stale = app
        .world_mut()
        .query::<(&WheelPuff, &SessionEntity)>()
        .iter(app.world())
        .filter(|(_, s)| s.0 != 2)
        .count();
    assert_eq!(stale, 0, "gen-1 puffs survived the restart");
    assert!(app.world().get_resource::<WheelFx>().is_some());
}

/// `ptxindex` beyond the eight-name table has no authored rule — the
/// slot resolves nothing and stays dark rather than clamping onto a
/// real effect.
#[test]
fn an_out_of_table_index_stays_dark() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    // Re-author testwater with an index past the table's end.
    write(
        tmp.path(),
        "city/materials.mtl",
        MATERIALS_MTL.replace("ptxindex: -1 6", "ptxindex: -1 9"),
    );
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let water = material_index(&mut app, "testwater");
    let surface = surface_tag(&mut app, water);
    let car = test_car(&mut app, 77);
    set_wheel(&mut app, car, 0, Some(surface), 0.8);
    run(&mut app, 30);
    assert_eq!(app.world().resource::<WheelFxReport>().emitted, 0);
}

/// `SurfaceTables::ptx_channels` reads what the tests bind —
/// `testwater`'s threshold-0 splash leg and `_default`'s inheritance
/// are authored data, verified against the loaded set.
#[test]
fn the_surface_table_resolves_the_authored_channels() {
    let tmp = city_install();
    write_wheel_fx(tmp.path());
    let mut app = city_app(vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let water_idx = material_index(&mut app, "testwater");
    let tables = app.world().resource::<mm2_content::SurfaceTables>();
    let water = tables
        .ptx_channels(SurfaceMaterial::Authored(water_idx))
        .unwrap();
    assert_eq!(
        water,
        PtxChannels {
            index: [-1, 6],
            threshold: [0.0, 0.0],
        }
    );
    let default = tables.ptx_channels(SurfaceMaterial::Unspecified).unwrap();
    assert_eq!(default.index, [4, -1]);
}

/// F06-AC05 end to end: one real city collider, one real car through
/// the Avian wheel path, and the three consumers — the tire force, the
/// skid voice and the wheel puffs — all read the same contact. A car
/// slid sideways across the authored `testgrass` road (re-authored
/// here to friction 0.6 so its grip is distinguishable from the
/// neutral 1.0) must (a) apply the `TireSurface` grip that surface
/// normalizes to, (b) resolve the `sound` class the table assigns the
/// same material and sound its authored skid band, and (c) emit the
/// dust/grass channels the same material authors — every consumer
/// naming the one `SurfaceMaterial` under the wheel.
#[test]
fn one_collider_drives_the_tire_the_voice_and_the_puff() {
    use mm2_app::audio::{SurfaceContact, SurfaceRole, SurfaceVoice};
    use mm2_game::PlayerVehicle;
    use mm2_vehicle::TireConditions;

    let tmp = city_install();
    write_wheel_fx(tmp.path());
    crate::support::surface_audio(tmp.path());
    write(
        tmp.path(),
        "city/materials.mtl",
        MATERIALS_MTL.replace(
            "friction: 1.0\neffect: none\nsound: 1",
            "friction: 0.6\neffect: none\nsound: 1",
        ),
    );
    let mut app = city_app(vfs_of(tmp.path()));
    app.insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.0, 10.0), 0.0));
    app.update();
    assert!(playing(&mut app));

    let grass = SurfaceMaterial::Authored(material_index(&mut app, "testgrass"));
    let tables = app.world().resource::<mm2_content::SurfaceTables>();
    let tire = tables
        .tire_surface_for(grass)
        .expect("authored tire surface");
    let sound = tables.sound_index(grass).expect("authored sound class");
    let ptx = tables.ptx_channels(grass).expect("authored ptx channels");
    assert!(tire.grip < 0.99, "the road is not the neutral surface");
    assert_eq!(sound, 1, "testgrass authors sound: 1 — the grass row");
    assert_eq!(ptx.index, [1, 2]);
    let traction = app
        .world()
        .get_resource::<TireConditions>()
        .map_or(1.0, |c| c.traction);

    let car = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .expect("one local car");
    // Settle onto the road, then slide the car sideways across the
    // strip — every wheel slips hard on the authored surface.
    run(&mut app, 60);
    let at = Vec3::new(-2.6, 0.7, 10.0);
    app.world_mut().entity_mut(car).insert((
        Position(at),
        Transform::from_translation(at),
        LinearVelocity(Vec3::new(6.0, 0.0, 0.0)),
    ));

    let (mut gripped, mut skidded, mut puffed, mut voiced) = (0, 0, 0, 0);
    let mut channels = std::collections::BTreeSet::new();
    for _ in 0..90 {
        app.update();
        let state = app.world().get::<VehicleState>(car).unwrap();
        for w in state.wheels.iter().filter(|w| w.grounded) {
            let on = w
                .contact_entity
                .and_then(|e| app.world().get::<SurfaceMaterial>(e))
                .copied();
            if on != Some(grass) {
                continue;
            }
            // (a) The tire path applied exactly the classified grip.
            assert!(
                (w.surface_grip - tire.grip * traction).abs() < 1e-5,
                "wheel grip {} vs {}",
                w.surface_grip,
                tire.grip * traction
            );
            gripped += 1;
        }
        // (b) The published contact names the sound class of a
        // material some grounded wheel actually rests on (the
        // sidewalk's unmarked collider reads the `_default` row).
        if let Some(skid) = app.world().get::<SurfaceContact>(car).and_then(|c| c.skid) {
            let tables = app.world().resource::<mm2_content::SurfaceTables>();
            let underfoot: Vec<u16> = state
                .wheels
                .iter()
                .filter(|w| w.grounded)
                .map(|w| {
                    let m = w
                        .contact_entity
                        .and_then(|e| app.world().get::<SurfaceMaterial>(e))
                        .copied()
                        .unwrap_or_default();
                    tables
                        .sound_index(m)
                        .expect("every material resolves a class")
                })
                .collect();
            assert!(
                underfoot.contains(&skid.surface),
                "{skid:?} vs {underfoot:?}"
            );
            if skid.surface == sound {
                skidded += 1;
            }
        }
        // The grass row's lone band (`grassskid`, 11025 Hz) is the
        // voice the class resolves — not the road rows' bands.
        let world = app.world_mut();
        let mut q = world.query::<(&SurfaceVoice, &AudioPlayer<mm2_app::audio::PcmAudio>)>();
        for (v, player) in q.iter(world) {
            let rate = world
                .resource::<Assets<mm2_app::audio::PcmAudio>>()
                .get(&player.0)
                .unwrap()
                .sample_rate
                .get();
            if v.role == SurfaceRole::Skid(0) {
                assert_eq!(rate, 11025);
                voiced += 1;
            }
        }
        // (c) Every puff's tile range (rule index × 8 in this
        // fixture) is a channel of a material some grounded wheel has
        // rested on — a puff outlives its contact, so the set only
        // grows — and the road's own dust/grass ranges must appear.
        let tables = app.world().resource::<mm2_content::SurfaceTables>();
        for w in app
            .world()
            .get::<VehicleState>(car)
            .unwrap()
            .wheels
            .iter()
            .filter(|w| w.grounded)
        {
            let m = w
                .contact_entity
                .and_then(|e| app.world().get::<SurfaceMaterial>(e))
                .copied()
                .unwrap_or_default();
            channels.extend(
                tables
                    .ptx_channels(m)
                    .unwrap()
                    .index
                    .into_iter()
                    .filter(|i| *i >= 0)
                    .map(|i| i * 8),
            );
        }
        for (_, frame_start, _) in car_puffs(&mut app, car) {
            assert!(channels.contains(&frame_start), "stray puff {frame_start}");
            if matches!(frame_start, 8 | 16) {
                puffed += 1;
            }
        }
    }
    assert!(gripped > 0, "a wheel never read the road collider");
    assert!(skidded > 0, "no skid contact resolved");
    assert!(voiced > 0, "no skid voice sounded");
    assert!(puffed > 0, "no puff emitted");
}

/// F06-AC02 end to end: two authored surfaces under a dry and a rainy
/// session, driven through the real `load_session_world` car and
/// Avian. The synthetic city's one texture is mapped to `testgrass`
/// (friction 0.6) or `testquiet` (1.0). For each (weather, surface)
/// pair a car slides sideways at 6 m/s; every grounded wheel's applied
/// grip must equal the authored surface grip × the session's weather
/// factor (applied once — not twice, not on the displayed value only),
/// and the slide speed still left shortly after must fall in the order
/// of that grip.
#[test]
fn weather_and_surface_move_traction_in_the_real_force_path() {
    use mm2_game::PlayerVehicle;
    use mm2_vehicle::TireConditions;

    let mut results: Vec<(f32, f32)> = Vec::new(); // (expected grip, slide speed left)
    for (weather, surface) in [
        (0, "testquiet"),
        (3, "testquiet"),
        (0, "testgrass"),
        (3, "testgrass"),
    ] {
        let tmp = city_install();
        write_wheel_fx(tmp.path());
        write(
            tmp.path(),
            "city/materials.mtl",
            MATERIALS_MTL.replace(
                "friction: 1.0\neffect: none\nsound: 1",
                "friction: 0.6\neffect: none\nsound: 1",
            ),
        );
        write(
            tmp.path(),
            "city/materials.csv",
            format!("texture,physics\ntest_road,{surface}\n"),
        );
        let mut app = city_app_in(vfs_of(tmp.path()), weather);
        app.insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.0, 10.0), 0.0));
        app.update();
        assert!(playing(&mut app));

        let traction = app.world().resource::<TireConditions>().traction;
        assert_eq!(
            traction,
            if weather == 3 {
                mm2_game::WET_TRACTION
            } else {
                1.0
            },
            "weather {weather}"
        );
        let material = SurfaceMaterial::Authored(material_index(&mut app, surface));
        let tables = app.world().resource::<mm2_content::SurfaceTables>();
        let expected = tables.tire_surface_for(material).expect("authored").grip * traction;

        let car = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerVehicle>>()
            .single(app.world())
            .expect("one local car");
        run(&mut app, 60);
        app.world_mut()
            .entity_mut(car)
            .insert(LinearVelocity(Vec3::new(6.0, 0.0, 0.0)));

        let (mut checked, mut left) = (0, 0.0);
        for frame in 0..6 {
            app.update();
            let state = app.world().get::<VehicleState>(car).unwrap();
            for w in state.wheels.iter().filter(|w| w.grounded) {
                let on = w
                    .contact_entity
                    .and_then(|e| app.world().get::<SurfaceMaterial>(e))
                    .copied();
                assert_eq!(on, Some(material), "{surface}: wheel off the strip");
                assert!(
                    (w.surface_grip - expected).abs() < 1e-5,
                    "{surface} weather {weather}: wheel grip {} vs {expected}",
                    w.surface_grip
                );
                checked += 1;
            }
            if frame == 5 {
                left = app.world().get::<LinearVelocity>(car).unwrap().0.x.abs();
            }
        }
        assert!(checked > 0, "no wheel was grounded");
        results.push((expected, left));
    }

    // The four grips are distinct …
    let mut grips: Vec<f32> = results.iter().map(|r| r.0).collect();
    grips.sort_by(f32::total_cmp);
    grips.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    assert_eq!(grips.len(), 4, "{results:?}");
    // … and less grip leaves more of the slide.
    results.sort_by(|a, b| b.0.total_cmp(&a.0));
    for pair in results.windows(2) {
        assert!(
            pair[1].1 > pair[0].1,
            "grip {} kept {} m/s of slide, grip {} kept {}",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
    }
}

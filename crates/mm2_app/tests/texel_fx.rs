//! F05-B.9 integration — the `fxTexelDamage` rig: `_dmg`-paired body
//! shaders bind a per-vehicle cloned texture at spawn, the deduplicated
//! impact stream stamps splats into it through `ApplyDamage`, and a
//! repair re-blits the clean texture. Self-authored fixtures only: a
//! two-triangle body bound to a synthetic TGA pair.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::car_visual;
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_app::texel_fx::{self, TexelDamageReport, TexelDamageRig};
use mm2_app::{contracts, damage};
use mm2_assets::Vfs;
use mm2_content::model::{Lod, MeshGroup, ModelPart, PartRole, VehicleModel};
use mm2_formats::pkg::PkgShader;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::VehCarDamage;
use mm2_game::{
    DamageEvent, DamageSpec, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player,
    PlayerControl, Session, SessionConfig, SessionPhase, SurfaceState, VehicleDamage,
    advance_session_tick,
};
use mm2_vehicle::{TireConditions, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// `VehicleConfig::default().mass` — impulse is `severity × MASS`.
const MASS: f32 = 1300.0;
/// The vpbug-shaped bounds the damage tests use.
const SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};

/// `vehcardamage` shaped like the retail records — the rig reads only
/// `TextelDamageRadius` (2.0 m here so both fixture tris qualify).
const CARDAMAGE: &str = "type: a\r\n\
vehCarDamage {\r\n\
  MaxDamage 321300.000000\r\n\
  MedDamage 150000.000000\r\n\
  ImpactThreshold 1500.000000\r\n\
  RegenerateRate 0.000000\r\n\
  SmokeOffset 0.100000 0.500000 -1.000000\r\n\
  TextelDamageRadius 2.000000\r\n\
  Position 0.000000 0.000000 0.000000\r\n\
  PositionVar 0.100000 0.000000 0.100000\r\n\
  Velocity 0.000000 1.000000 0.000000\r\n\
  VelocityVar 0.500000 0.000000 0.500000\r\n\
  Life 1.000000\r\n\
  LifeVar 0.500000\r\n\
  Mass 1.000000\r\n\
  MassVar 0.000000\r\n\
  Radius 0.500000\r\n\
  RadiusVar 0.250000\r\n\
  Drag 0.500000\r\n\
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

fn damage_record() -> VehCarDamage {
    VehCarDamage::from_tune(&TuneFile::parse(CARDAMAGE).unwrap()).unwrap()
}

/// Uncompressed 32bpp top-left-origin TGA of a solid colour.
fn tga(w: u16, h: u16, px: [u8; 4]) -> Vec<u8> {
    let mut v = vec![0u8; 18];
    v[2] = 2; // uncompressed true-color
    v[12..14].copy_from_slice(&w.to_le_bytes());
    v[14..16].copy_from_slice(&h.to_le_bytes());
    v[16] = 32;
    v[17] = 8 | 0x20;
    for _ in 0..usize::from(w) * usize::from(h) {
        v.extend_from_slice(&[px[2], px[1], px[0], px[3]]); // BGRA
    }
    v
}

/// A dir-mounted VFS with a `car_paint`/`car_paint_dmg` pair in
/// distinct colours plus one unpaired `solo` texture.
struct Fixture {
    _dir: tempfile::TempDir,
    vfs: Vfs,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(
        root.join("texture/car_paint.tga"),
        tga(64, 64, [10, 10, 200, 255]),
    )
    .unwrap();
    std::fs::write(
        root.join("texture/car_paint_dmg.tga"),
        tga(64, 64, [200, 10, 10, 255]),
    )
    .unwrap();
    std::fs::write(root.join("texture/solo.tga"), tga(8, 8, [0, 0, 0, 255])).unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();
    Fixture { _dir: dir, vfs }
}

fn shader(texture: &str) -> PkgShader {
    PkgShader {
        texture: texture.to_string(),
        diffuse: [1.0; 4],
        ambient: [1.0; 4],
        specular: None,
        emissive: [0.0; 4],
        shininess: 0.0,
    }
}

/// A `body` part whose high LOD is two triangles on the ground plane —
/// one per shader slot, `car_paint_dmg` on slot 0, unpaired `solo` on 1.
fn model() -> VehicleModel {
    let group = |offset: usize, x: f32| MeshGroup {
        shader_offset: offset,
        positions: vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 0.0, 1.0]],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        uvs: vec![[0.5, 0.5], [0.55, 0.5], [0.5, 0.55]],
        indices: vec![0, 1, 2],
    };
    VehicleModel {
        parts: vec![ModelPart {
            name: "body".into(),
            role: PartRole::Body,
            lods: vec![(Lod::H, vec![group(0, 0.0), group(1, 10.0)])],
            origin: None,
            pivot: None,
            recenter: None,
        }],
        paint_jobs: 1,
        shaders_per_paint_job: 2,
        shaders: vec![shader("car_paint_dmg"), shader("solo")],
        ..VehicleModel::default()
    }
}

/// Playing session + the texel pipeline; spawns the fixture model under
/// a player-marked car and returns app/car/object/vfs.
fn texel_app(pos: Vec3) -> (App, Entity, ObjectId, Fixture) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let object = session.mint_object_id();
    let player = session.mint_player_id();
    let role = session.authority_role();

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
        .insert_resource(TireConditions::default())
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<DamageEvent>()
        .insert_resource(contracts::ImpactFilter::default())
        .init_resource::<damage::DamageReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<TexelDamageReport>()
        .init_resource::<SessionControl>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(SpawnPoint {
            position: pos,
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                damage::apply_impact_damage,
                texel_fx::apply_texel_damage,
                damage::resolve_disabled,
            )
                .chain(),
        );
    app.finish();
    app.cleanup();

    let fixture = fixture();
    let car = app
        .world_mut()
        .spawn((
            mm2_game::PlayerVehicle,
            ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleDamage::new(SPEC),
            vehicle_bundle(&VehicleConfig::default()),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id();

    // Spawn the model through the production path — rig build included.
    // `Commands` borrows the world immutably, so the asset stores are
    // swapped out for the call and put back after the queue applies.
    let model = model();
    let damage = damage_record();
    let (mut images, mut materials, mut meshes) = {
        let world = app.world_mut();
        (
            std::mem::take(&mut *world.resource_mut::<Assets<Image>>()),
            std::mem::take(&mut *world.resource_mut::<Assets<StandardMaterial>>()),
            std::mem::take(&mut *world.resource_mut::<Assets<Mesh>>()),
        )
    };
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let missing = {
        let world = app.world_mut();
        let mut commands = Commands::new(&mut queue, world);
        car_visual::spawn_vehicle_model(
            &mut commands,
            &fixture.vfs,
            &model,
            0,
            &mut meshes,
            &mut images,
            &mut materials,
            car,
            Some((&damage, 42)),
        )
    };
    assert!(missing.is_empty(), "fixture textures resolve: {missing:?}");
    queue.apply(app.world_mut());
    {
        let world = app.world_mut();
        *world.resource_mut::<Assets<Image>>() = images;
        *world.resource_mut::<Assets<StandardMaterial>>() = materials;
        *world.resource_mut::<Assets<Mesh>>() = meshes;
    }
    app.update();
    (app, car, object, fixture)
}

#[test]
fn the_rig_binds_paired_slots_and_skips_unpaired_ones() {
    let (app, car, _object, _f) = texel_app(Vec3::new(0.0, 1.2, 0.0));
    let rig = app
        .world()
        .get::<TexelDamageRig>(car)
        .expect("a _dmg-paired body builds the rig");
    assert_eq!(rig.slots.len(), 1, "only slot 0 pairs a damage texture");
    assert_eq!(rig.radius, 2.0);
    assert_eq!(rig.mesh.tris.len(), 1, "only the paired group's tris");
    // A body child binds the per-vehicle clone — the unpaired `solo`
    // group keeps the shared material (not another clone).
    let bound: Vec<Handle<StandardMaterial>> = app
        .world()
        .iter_entities()
        .filter_map(|e| {
            e.get::<MeshMaterial3d<StandardMaterial>>()
                .map(|m| m.0.clone())
        })
        .collect();
    assert_eq!(bound.len(), 2, "one mesh child per group");
    assert!(
        bound.contains(&rig.slots[0].material),
        "the paired group binds the per-vehicle material"
    );
}

#[test]
fn an_impact_splats_the_clone_and_a_repair_restores_it() {
    let (mut app, car, object, _f) = texel_app(Vec3::new(0.0, 1.2, 0.0));
    let rig = app.world().get::<TexelDamageRig>(car).unwrap();
    let (current, clean) = (rig.slots[0].current.clone(), rig.slots[0].clean.clone());
    let clean_data = app
        .world()
        .resource::<Assets<Image>>()
        .get(&clean)
        .unwrap()
        .data
        .clone()
        .unwrap();
    // The clone starts clean.
    assert_eq!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&current)
            .unwrap()
            .data
            .as_deref(),
        Some(clean_data.as_slice())
    );

    // An impact on the damage-slot triangle — world point = car pos +
    // the tri's car-space location (the car sits at y=1.2 with no
    // rotation, so local ≈ world − pos; the tri is at the origin arm).
    let (generation, tick) = {
        let s = app.world().resource::<Session>();
        (s.generation(), s.tick())
    };
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(1),
            generation,
            tick,
            participants: (object, ObjectId::WORLD),
            // Near the slot-0 tri's verts in car space (the rig is at
            // y≈1.2 world; the tri's local y is 0 → the world point is
            // pos + tri point).
            point: Vec3::new(0.2, 1.2, 0.2),
            normal: Vec3::Y,
            severity: 10.0,
            surface: SurfaceState::default(),
        });
    app.update();

    let report = app.world().resource::<TexelDamageReport>();
    assert_eq!(report.impacts, 1, "one splatting impact");
    assert_eq!(report.splats, 1, "one tri in radius → one splat");
    let damaged_data = app
        .world()
        .resource::<Assets<Image>>()
        .get(&current)
        .unwrap()
        .data
        .clone()
        .unwrap();
    assert_ne!(
        damaged_data, clean_data,
        "the splat stamped damage texels onto the clone"
    );
    // The shared clean texture is untouched.
    assert_eq!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&clean)
            .unwrap()
            .data
            .as_deref(),
        Some(clean_data.as_slice())
    );

    // A disabling hit repairs through `resolve_disabled` — the skin
    // follows the mechanical reset (`fxTexelDamage::Reset` at the
    // damage.reset() site, DMG-9's retail pairing).
    let tick2 = app.world().resource::<Session>().tick();
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(2),
            generation,
            tick: tick2,
            participants: (object, ObjectId::WORLD),
            point: Vec3::new(0.2, 1.2, 0.2),
            normal: Vec3::Y,
            // severity × MASS lands past MaxDamage → Disabled → the
            // cruise free-reset repairs.
            severity: (SPEC.max_damage / MASS) + 1.0,
            surface: SurfaceState::default(),
        });
    app.update();
    let report = app.world().resource::<TexelDamageReport>();
    assert_eq!(report.resets, 1, "the repair re-blitted the clean texture");
    assert_eq!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&current)
            .unwrap()
            .data
            .as_deref(),
        Some(clean_data.as_slice()),
        "reset re-blits the clean texture"
    );
}

#[test]
fn a_car_without_a_damage_record_or_pairs_carries_no_rig() {
    // No authored `vehcardamage` → `texel_damage: None` → no rig even
    // though the model's shader pairs a `_dmg` texture.
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(session);
    let fixture = fixture();
    let car = app.world_mut().spawn_empty().id();
    let model = model();
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
        let world = app.world_mut();
        let mut commands = Commands::new(&mut queue, world);
        car_visual::spawn_vehicle_model(
            &mut commands,
            &fixture.vfs,
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
    assert!(
        app.world().get::<TexelDamageRig>(car).is_none(),
        "no authored record → no rig"
    );
}

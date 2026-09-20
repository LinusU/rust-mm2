//! F06-A surface identity through the real physics query path: a
//! wheel-style downward ray into each region of a multi-surface city
//! reports that region's authored material (F06-AC01's collider leg —
//! the telemetry half is covered by `tests/contracts.rs`).
//!
//! Synthetic install: one room whose road, sidewalk and ground-fan
//! textures resolve to two named materials plus an unmapped name —
//! no original data.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::load_city;
use mm2_assets::Vfs;
use mm2_content::surface::{CSV_PATH, MTL_PATH};
use mm2_game::{CityEntity, SessionEntity, SurfaceMaterial};

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
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

/// One room: a two-section road under `test_road` (sidewalks take the
/// next slot, `test_grass`) and a ground fan under `mystery`, a name
/// the tables do not know. Same shape `import_pipeline` uses.
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

    // Texture table stores count + 1 → three names.
    d.extend_from_slice(&4u32.to_le_bytes());
    push_lp(&mut d, "test_road");
    push_lp(&mut d, "test_grass");
    push_lp(&mut d, "mystery");

    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions

    let mut attr_words: Vec<u16> = Vec::new();
    let attr = |words: &mut Vec<u16>, word: u16, data: &[u16]| {
        words.push(word);
        words.extend_from_slice(data);
    };
    attr(&mut attr_words, 0x0a << 3, &[1]); // texture ref → textures[0]
    attr(&mut attr_words, 0x00, &[2, 0, 1, 2, 3, 4, 5, 6, 7]); // counted road
    attr(&mut attr_words, 0x0a << 3, &[3]); // texture ref → textures[2]
    attr(&mut attr_words, 0x06 << 3, &[2, 8, 9, 10, 11]); // counted fan

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

    d.extend_from_slice(&[0u8; 2]); // room flags
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[-5., 0., 0.]);
    push_f32s(&mut d, &[20., 2., 20.]);
    push_f32s(&mut d, &[7., 1., 10.]);
    push_f32s(&mut d, &[30.]);
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

const SURF_MTL: &str = "\
mtl _default {
    elasticity: 0.0
    friction: 1.0
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl cobblestone {
    elasticity: 0.1
    friction: 0.9
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl grass {
    elasticity: 0.4
    friction: 0.7
    effect: none
    sound: 2
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
";

const SURF_CSV: &str = "\
texture,physics
test_road,cobblestone
test_grass,grass
";

/// `Vfs` is not a `Resource` — a thin wrapper carries it in.
#[derive(Resource)]
struct VfsRes(Vfs);

/// A physics-enabled app world holding the synthetic city: the same
/// plugin set `tests/banger.rs` uses, minus the session schedule —
/// the collider entities are what matter here.
fn city_app(vfs: Vfs) -> App {
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
        .add_plugins(TransformPlugin)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(VfsRes(vfs));
    app.finish();
    app.cleanup();

    // `load_city` is a Commands-side producer: run it against the
    // world, apply, then let Avian index the new colliders.
    app.world_mut()
        .run_system_once(
            |mut commands: Commands,
             vfs: Res<VfsRes>,
             mut meshes: ResMut<Assets<Mesh>>,
             mut images: ResMut<Assets<Image>>,
             mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut session = mm2_game::Session::new();
                let loaded = load_city(
                    &mut commands,
                    &vfs.0,
                    "city/test.psdl",
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                    &mut session,
                )
                .expect("city loads");
                assert_eq!(loaded.report.surfaces.named, 2);
                assert_eq!(loaded.report.surfaces.unmapped.len(), 1);
                if let Some(tables) = loaded.surfaces {
                    commands.insert_resource(tables);
                }
            },
        )
        .expect("load_city system runs");
    for _ in 0..3 {
        app.update();
    }
    app
}

/// The `SurfaceMaterial` under `x, z`, via the same downward ray the
/// wheels cast.
fn surface_at(app: &mut App, x: f32, z: f32) -> Option<SurfaceMaterial> {
    app.world_mut()
        .run_system_once(
            move |spatial: SpatialQuery, mats: Query<&SurfaceMaterial>| {
                spatial
                    .cast_ray(
                        Vec3::new(x, 5.0, z),
                        Dir3::NEG_Y,
                        20.0,
                        true,
                        &SpatialQueryFilter::default(),
                    )
                    .map(|hit| mats.get(hit.entity).copied().unwrap_or_default())
            },
        )
        .expect("raycast system runs")
}

#[test]
fn a_ray_into_each_region_reports_its_authored_surface() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "city/test.psdl", city_psdl());
    write(root, MTL_PATH, SURF_MTL);
    write(root, CSV_PATH, SURF_CSV);
    for name in ["test_road", "test_grass", "mystery"] {
        write(
            root,
            &format!("texture/{name}.png"),
            include_bytes!("../../../assets/texture/dev_road.png"),
        );
    }
    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut app = city_app(vfs);

    // The session resource carries the index space.
    let tables = app
        .world()
        .get_resource::<mm2_content::SurfaceTables>()
        .expect("surface tables are a session resource");
    assert_eq!(tables.set.defs[1].name, "cobblestone");
    assert_eq!(tables.set.defs[2].name, "grass");

    // Three colliders, each marked — the road strip (|x| ≤ 3) reads
    // cobblestone, the kerb (3 < |x| ≤ 5) reads grass, the fan on the
    // unmapped name reports the conservative unspecified surface.
    let marked = app
        .world_mut()
        .query_filtered::<&SurfaceMaterial, With<CityEntity>>()
        .iter(app.world())
        .count();
    assert_eq!(marked, 3);
    assert_eq!(
        surface_at(&mut app, 0.0, 5.0),
        Some(SurfaceMaterial::Authored(1)),
        "road strip"
    );
    assert_eq!(
        surface_at(&mut app, 4.0, 5.0),
        Some(SurfaceMaterial::Authored(2)),
        "sidewalk"
    );
    assert_eq!(
        surface_at(&mut app, 15.0, 5.0),
        Some(SurfaceMaterial::Unspecified),
        "fan on an unmapped name"
    );
}

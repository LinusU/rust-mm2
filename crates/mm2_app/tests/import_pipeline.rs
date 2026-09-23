//! Synthetic end-to-end import tests: VFS bytes → parse → meshes, colliders,
//! materials, props — with no MM2 installation required.

use bevy::asset::Assets;
use bevy::ecs::query::With;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::Mesh;
use bevy::pbr::StandardMaterial;
use mm2_app::city::{emit_psdl, load_city, load_image, load_image_sequence};
use mm2_assets::Vfs;
use mm2_content::{SurfaceTables, surface};
use mm2_formats::materials::{MaterialMap, MaterialSet};
use mm2_formats::psdl::Psdl;
use mm2_game::{CityEntity, SessionEntity, SurfaceMaterial};

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

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

/// One-room city exercising every supported attribute family:
///
/// - a two-section road with sidewalks (counted form),
/// - a counted generic fan (ground patch),
/// - a facade, a facade collision bound and a sliver on one wall edge,
/// - a roof fan with a height override,
/// - a road-tunnel attribute (decoded, applies to following roads — it is
///   last, so it contributes no wall geometry but is still emitted).
fn synthetic_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size

    // Vertex pool. Road sections at z=0 and z=20; a ground fan, a wall
    // edge and a roof quad off to the side.
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
        [30., 0., 0.],
        [30., 0., 10.], // wall edge
        [35., 5., 0.],
        [45., 5., 0.],
        [45., 5., 10.],
        [35., 5., 10.], // roof
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }

    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);

    // Texture table stores count + 1.
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");

    // One real room (n_rooms - 1 records are stored).
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
    attr(&mut attr_words, (0x0b << 3) | 6, &[1, 2, 3, 2, 12, 13]); // facade
    attr(&mut attr_words, (0x07 << 3) | 4, &[0, 2, 12, 13]); // facade bound
    attr(&mut attr_words, (0x0c << 3) | 3, &[2, 14, 15, 16, 17]); // roof fan
    attr(&mut attr_words, (0x03 << 3) | 4, &[2, 0, 12, 13]); // sliver
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

/// A three-texture room for the F06-A surface tests: a road under
/// `test_road` (its sidewalks take the next slot, `test_grass`), and a
/// ground fan under `mystery` — a name the surface tables cannot
/// classify.
fn two_surface_psdl() -> Vec<u8> {
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

    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules

    push_f32s(&mut d, &[-5., 0., 0.]); // bounds min
    push_f32s(&mut d, &[20., 2., 20.]); // bounds max
    push_f32s(&mut d, &[7., 1., 10.]); // bounds centre
    push_f32s(&mut d, &[30.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// The `city/materials.{mtl,csv}` pair the surface tests share:
/// `test_road` → `cobblestone` (index 1), `test_grass` → `grass`
/// (index 2); `mystery` has no row.
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

fn surface_tables() -> SurfaceTables {
    SurfaceTables {
        set: MaterialSet::parse(SURF_MTL).expect("mtl parses"),
        map: MaterialMap::parse(SURF_CSV).expect("csv parses"),
    }
}

/// INST file placing one `testprop` PKG at (40, 0, 5) via the simple form.
fn synthetic_inst() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&1u16.to_le_bytes()); // room
    d.extend_from_slice(&0u16.to_le_bytes()); // modifiers
    let name = b"testprop\0";
    d.push(0x80 | name.len() as u8); // simple placement
    d.extend_from_slice(name);
    push_f32s(&mut d, &[1.0, 0.0, 40.0, 0.0, 5.0]); // x_delta, z_delta, xyz
    d
}

/// One `PTH1` path record: `name`, `points`, raw `kind`/`spacing` bytes.
fn pth1_path(name: &str, points: &[[f32; 3]], kind: u8, spacing: u8) -> Vec<u8> {
    let mut d = vec![0u8; 32];
    d[..name.len()].copy_from_slice(name.as_bytes());
    d.extend_from_slice(&(points.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // selection
    for p in points {
        d.extend_from_slice(&0u32.to_le_bytes()); // attributes
        push_f32s(&mut d, p);
    }
    d.push(kind);
    d.push(spacing);
    d.extend_from_slice(&[0, 0]);
    d
}

/// A `PTH1` file from path records.
fn pth1(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut d = b"PTH1".to_vec();
    d.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    for p in paths {
        d.extend_from_slice(p);
    }
    d
}

/// A 2×2 P8 TEX: palette entry 1 is red at alpha 0x40 — the decal
/// decode must surface it (palette-alpha honoring) where ordinary
/// decodes would report opaque.
fn synthetic_decal_tex() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&2u16.to_le_bytes()); // w
    d.extend_from_slice(&2u16.to_le_bytes()); // h
    d.extend_from_slice(&1u16.to_le_bytes()); // P8
    d.extend_from_slice(&1u16.to_le_bytes()); // mips
    d.extend_from_slice(&0u16.to_le_bytes()); // unknown
    d.extend_from_slice(&0u32.to_le_bytes()); // bits
    for i in 0..256usize {
        if i == 1 {
            d.extend_from_slice(&[0, 0, 255, 0x40]); // BGRA
        } else {
            d.extend_from_slice(&[0, 0, 0, 0xff]);
        }
    }
    d.extend_from_slice(&[1, 1, 1, 1]); // pixels
    d
}

/// One quad road room (road x 2–28, building lines x 0/30, z 0–20)
/// carrying `prop_rule` byte 1 and one single-room prop path — the
/// minimum PSDL that exercises the prop-rule walk end to end.
fn proprule_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[
        [0., 0., 0.],
        [2., 0., 0.],
        [28., 0., 0.],
        [30., 0., 0.], // entry run (z = 0): outer, curb, curb, outer
        [30., 0., 20.],
        [28., 0., 20.],
        [2., 0., 20.],
        [0., 0., 20.], // exit run (z = 20)
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    d.extend_from_slice(&1u32.to_le_bytes()); // heights
    push_f32s(&mut d, &[0.15]);
    d.extend_from_slice(&2u32.to_le_bytes()); // textures (count + 1)
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut attr_words: Vec<u16> = Vec::new();
    attr_words.push(0x0a << 3); // texture ref → textures[0]
    attr_words.push(1);
    attr_words.push(0x00); // counted road, 2 sections × 4 refs
    attr_words.extend_from_slice(&[2, 0, 1, 2, 3, 7, 6, 5, 4]);
    let mut room = Vec::new();
    room.extend_from_slice(&8u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in 0u16..8 {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // no neighbours
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags
    d.extend_from_slice(&[0u8, 1]); // prop rules: room 1 → n01
    push_f32s(&mut d, &[0., 0., 0.]);
    push_f32s(&mut d, &[30., 1., 20.]);
    push_f32s(&mut d, &[15., 0., 10.]);
    push_f32s(&mut d, &[25.]);
    // One prop path: curb pairs (v1,v2) entry / (v5,v6) exit, room 1.
    d.extend_from_slice(&1u32.to_le_bytes()); // nPaths
    d.extend_from_slice(&0u16.to_le_bytes()); // unknown4
    d.extend_from_slice(&0u16.to_le_bytes()); // unknown5
    d.push(0); // n_f
    d.push(0); // n_b
    d.extend_from_slice(&0u16.to_le_bytes()); // unknown6
    for v in [1u16, 2, 0, 0] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    for v in [5u16, 6, 0, 0] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.push(1); // n_rooms
    d.extend_from_slice(&1u16.to_le_bytes()); // room 1
    d
}

/// PKG3 with one tetrahedron geometry chunk (`testprop_h`), no shaders.
fn synthetic_pkg() -> Vec<u8> {
    // Geometry payload.
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&12u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    for (p, n, uv) in [
        ([0., 0., 0.], [0., 1., 0.], [0., 0.]),
        ([1., 0., 0.], [0., 1., 0.], [1., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [0., 1.]),
        ([0., 1., 0.], [0., 1., 0.], [0.5, 0.5]),
    ] {
        push_f32s(&mut geo, &p);
        push_f32s(&mut geo, &n);
        push_f32s(&mut geo, &uv);
    }
    geo.extend_from_slice(&12u32.to_le_bytes()); // strip indices
    for i in [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3] {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut d = Vec::new();
    d.extend_from_slice(b"PKG3");
    d.extend_from_slice(b"FILE");
    push_lp(&mut d, "testprop_h");
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn emits_counted_attributes_meshes_colliders_and_report() {
    let bytes = synthetic_psdl();
    let psdl = Psdl::parse(&bytes).expect("synthetic PSDL parses");
    assert_eq!(psdl.rooms.len(), 1);
    assert_eq!(psdl.rooms[0].attributes.len(), 8);

    let import = emit_psdl(&psdl, None);

    // Every attribute produces geometry, collision or tunnel state.
    assert_eq!(import.report.attributes, 8);
    assert_eq!(import.report.emitted, 7);
    assert!(import.report.unsupported.is_empty());
    assert_eq!(import.report.rejected, 0);
    assert_eq!(import.report.suppressed, 0);

    // Two material groups for the room: the road/fan/facade texture and
    // the sidewalk texture slot (which falls back — only one texture is
    // named in the file).
    assert_eq!(import.meshes.len(), 2);
    assert!(import.meshes.iter().all(|g| g.room == 0));
    assert!(import.meshes.iter().all(|g| !g.indices.is_empty()));

    // The road strip, sidewalks, fan, facade bound and roof all feed one
    // static collider for the room.
    assert_eq!(import.colliders.len(), 1);
    let collider = &import.colliders[0];
    assert!(collider.tris.len() >= 10, "collider has all surfaces");

    // Spawn sits on the road's midline (y = 0) with the configured
    // clearance, heading along the road. The synthetic road runs from
    // z = 0 to z = 20 and coordinates are used as authored, so the
    // midpoint keeps its +z.
    assert!(
        (import.spawn - bevy::math::Vec3::new(0.0, 1.5, 10.0)).length() < 0.01,
        "spawn {:?}",
        import.spawn
    );
    let forward = bevy::math::Quat::from_rotation_y(import.spawn_yaw) * bevy::math::Vec3::NEG_Z;
    assert!(
        forward.x.abs() < 1e-4 && forward.z.abs() > 0.99,
        "{forward:?}"
    );
}

/// F06-A: the room's collider splits per authored surface class — the
/// road strip, its sidewalk-texture slot and the unmapped fan texture
/// become three colliders carrying `Authored(i)`/`Unspecified`, and
/// the report names what fell back. Without tables the pre-F06 single
/// `Unspecified` collider per room is preserved.
#[test]
fn emit_psdl_groups_colliders_by_authored_surface() {
    let psdl = Psdl::parse(&two_surface_psdl()).expect("synthetic PSDL parses");
    let tables = surface_tables();

    let import = emit_psdl(&psdl, Some(&tables));
    assert_eq!(import.colliders.len(), 3, "one collider per surface class");
    assert!(import.colliders.iter().all(|c| c.room == 0));
    let by_surface = |s: SurfaceMaterial| {
        import
            .colliders
            .iter()
            .find(|c| c.surface == s)
            .unwrap_or_else(|| panic!("a collider carries {s:?}"))
    };
    let road = by_surface(SurfaceMaterial::Authored(1));
    let walk = by_surface(SurfaceMaterial::Authored(2));
    let fan = by_surface(SurfaceMaterial::Unspecified);
    // The groups split on the authored material, not on geometry:
    // road tris span |x| ≤ 3, sidewalk kerbs |x| ∈ 3..5, the fan x > 9.
    let xs = |c: &mm2_app::city::RoomCollider| c.positions.iter().map(|p| p.x).collect::<Vec<_>>();
    assert!(xs(road).iter().all(|x| x.abs() <= 3.01));
    assert!(xs(walk).iter().all(|x| (3.0..=5.01).contains(&x.abs())));
    assert!(xs(fan).iter().all(|x| *x > 9.0));

    let s = &import.report.surfaces;
    assert!(s.loaded);
    assert_eq!(s.named, 2);
    assert_eq!(s.none, 0);
    assert_eq!(s.blank, 0);
    assert_eq!(
        s.unmapped.iter().map(String::as_str).collect::<Vec<_>>(),
        ["mystery"]
    );

    // No tables: one default collider per room, exactly as before F06.
    let import = emit_psdl(&psdl, None);
    assert_eq!(import.colliders.len(), 1);
    assert_eq!(import.colliders[0].surface, SurfaceMaterial::Unspecified);
    assert!(!import.report.surfaces.loaded);
    assert!(import.report.surfaces.failure.is_none());
}

/// End to end through `load_city`: the VFS-resolved tables classify
/// the texture table, every spawned collider entity carries the
/// `SurfaceMaterial` wheel raycasts read, and `LoadedCity` hands the
/// session its index space.
#[test]
fn vfs_to_city_marks_colliders_with_their_authored_surfaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), two_surface_psdl()).unwrap();
    std::fs::write(root.join(surface::MTL_PATH), SURF_MTL).unwrap();
    std::fs::write(root.join(surface::CSV_PATH), SURF_CSV).unwrap();
    for name in ["test_road", "test_grass", "mystery"] {
        std::fs::write(
            root.join(format!("texture/{name}.png")),
            include_bytes!("../../../assets/texture/dev_road.png"),
        )
        .unwrap();
    }

    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();

    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    // The session's surface identity space survived the load.
    let tables = loaded.surfaces.expect("tables loaded");
    assert_eq!(tables.set.defs[1].name, "cobblestone");
    assert_eq!(tables.set.defs[2].name, "grass");
    assert_eq!(loaded.report.surfaces.named, 2);
    assert_eq!(loaded.report.surfaces.unmapped.len(), 1);

    // Three collider entities, each carrying the component the
    // contact pipeline queries — and no others.
    let mut q = world.query_filtered::<&SurfaceMaterial, With<CityEntity>>();
    let mut surfaces: Vec<SurfaceMaterial> = q.iter(&world).copied().collect();
    surfaces.sort_by_key(|s| match s {
        SurfaceMaterial::Unspecified => 0,
        SurfaceMaterial::Authored(i) => i + 1,
    });
    assert_eq!(
        surfaces,
        vec![
            SurfaceMaterial::Unspecified,
            SurfaceMaterial::Authored(1),
            SurfaceMaterial::Authored(2),
        ]
    );
}

/// A present-but-broken table pair warns and falls back — the city
/// still loads, every collider is `Unspecified`, and the report
/// records the failure rather than hiding it (F06-AC04).
#[test]
fn vfs_to_city_with_a_broken_table_pair_marks_everything_unspecified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), two_surface_psdl()).unwrap();
    std::fs::write(root.join(surface::MTL_PATH), "garbage {").unwrap();
    std::fs::write(root.join(surface::CSV_PATH), SURF_CSV).unwrap();
    for name in ["test_road", "test_grass", "mystery"] {
        std::fs::write(
            root.join(format!("texture/{name}.png")),
            include_bytes!("../../../assets/texture/dev_road.png"),
        )
        .unwrap();
    }

    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();

    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    assert!(loaded.surfaces.is_none());
    assert!(!loaded.report.surfaces.loaded);
    assert!(loaded.report.surfaces.failure.is_some());

    let mut q = world.query_filtered::<&SurfaceMaterial, With<CityEntity>>();
    let surfaces: Vec<SurfaceMaterial> = q.iter(&world).copied().collect();
    assert_eq!(surfaces, vec![SurfaceMaterial::Unspecified]);
}

#[test]
fn vfs_to_city_spawns_meshes_colliders_and_props() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("geometry")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), synthetic_psdl()).unwrap();
    std::fs::write(root.join("city/test.inst"), synthetic_inst()).unwrap();
    std::fs::write(root.join("geometry/testprop.pkg"), synthetic_pkg()).unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();

    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();

    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    // 2 mesh groups + 1 room collider + 1 prop part + 1 prop collider
    // = 5 city entities.
    let city_entities = world.query::<&CityEntity>().iter(&world).count();
    assert_eq!(city_entities, 5);
    assert_eq!(loaded.report.props_spawned, 1);
    assert_eq!(loaded.report.props_failed, 0);
    assert!(loaded.report.missing_textures.is_empty());

    // The PSDL texture resolved to the PNG in the mount — at least one
    // material must carry a base colour texture.
    assert!(
        materials
            .iter()
            .any(|(_, m)| m.base_color_texture.is_some())
    );
}

/// The prop-rule channel end to end: `prop_rule` byte →
/// `proprules.csv` rows → `propdefs.csv` spacing → stamped props,
/// all through the real `load_city` path and the VFS.
#[test]
fn vfs_to_city_stamps_prop_rule_props() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city/test")).unwrap();
    std::fs::create_dir_all(root.join("geometry")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), proprule_psdl()).unwrap();
    std::fs::write(root.join("geometry/testprop.pkg"), synthetic_pkg()).unwrap();
    std::fs::write(
        root.join("city/test/propdefs.csv"),
        "name,start,distance,maxUse,minLerp,maxLerp,file1,file2,file3,file4\n\
         meter,5,10,99,0.5,0.5,testprop\n\
         lamp,2,6,2,0.5,0.5,testprop\n",
    )
    .unwrap();
    std::fs::write(
        root.join("city/test/proprules.csv"),
        "rulename,prop1,prop2,prop3,prop4,prop5,prop6,prop7,prop8\n\
         n01left,meter\n\
         n01right,lamp\n",
    )
    .unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();

    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();

    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    // lamp on the right side (start 2, dist 6, maxUse 2 → s = 2, 8);
    // meter on the left walked exit→entry (start 5, dist 10 → 5 m and
    // 15 m back from z = 20) — four stamps, all unbound static props.
    assert_eq!(loaded.report.proprule_rooms, 1);
    assert_eq!(loaded.report.proprule_stamps, 4);
    assert_eq!(loaded.report.proprule_bangers, 0);
    assert_eq!(loaded.report.proprule_unresolved, 0);
    assert_eq!(loaded.report.proprule_issues, 0);
    let stamped = world
        .query::<&bevy::prelude::Name>()
        .iter(&world)
        .filter(|n| n.as_str().starts_with("proprule-"))
        .count();
    assert_eq!(stamped, 8, "4 render parts + 4 colliders");
}

/// The decal channel end to end: `decals.pathset` → ribbon quads on a
/// merged `decal-<stem>` entity, texture resolved through the VFS,
/// palette-alpha honored → blended material, no collider — plus every
/// non-decal path class landing in the report.
#[test]
fn vfs_to_city_stamps_decal_ribbons() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city/test")).unwrap();
    std::fs::create_dir_all(root.join("geometry")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), synthetic_psdl()).unwrap();
    std::fs::write(root.join("geometry/testprop.pkg"), synthetic_pkg()).unwrap();
    std::fs::write(root.join("texture/testdecal.tex"), synthetic_decal_tex()).unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    std::fs::write(
        root.join("city/test/decals.pathset"),
        pth1(&[
            // A 4-point ribbon and an odd-tailed one — same stem,
            // merged into one entity: 1 + 2 = 3 quads.
            pth1_path(
                "testdecal",
                &[
                    [0.0, 0.0, 0.0],
                    [2.0, 0.0, 0.0],
                    [0.0, 0.0, 10.0],
                    [2.0, 0.0, 10.0],
                ],
                2,
                20,
            ),
            pth1_path(
                "testdecal",
                &[
                    [0.0, 0.0, 0.0],
                    [2.0, 0.0, 0.0],
                    [0.0, 0.0, 10.0],
                    [2.0, 0.0, 10.0],
                    [0.0, 0.0, 20.0],
                    [2.0, 0.0, 20.0],
                    [9.0, 9.0, 9.0],
                ],
                2,
                20,
            ),
            pth1_path("PATH01", &[[0.0, 0.0, 0.0]], 0, 0),
            pth1_path("testprop", &[[0.0, 0.0, 0.0]], 0, 0),
            pth1_path("missingtex", &[[0.0, 0.0, 0.0]], 0, 0),
            // Empty and under-4-point paths on a resolved stem reach
            // the empty/degenerate classes.
            pth1_path("testdecal", &[], 2, 20),
            pth1_path("testdecal", &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]], 2, 20),
        ]),
    )
    .unwrap();

    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();

    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();

    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    let d = &loaded.report.decals;
    assert_eq!(d.ribbons, 2);
    assert_eq!(d.quads, 3, "1 + 2 (odd tail dropped)");
    assert_eq!(d.entities, 1, "both ribbons merge under one texture stem");
    assert_eq!(d.label_paths, 1);
    assert_eq!(d.prop_paths, 1, "PKG-named path classified, not stamped");
    assert_eq!(d.unresolved_paths, 1);
    assert_eq!(d.empty_paths, 1);
    assert_eq!(d.degenerate_paths, 1, "the two-point path has no extent");
    assert_eq!(d.odd_point_paths, 1);
    assert_eq!(d.missing_textures, 0);

    // One merged, render-only decal entity — mesh + material, no body.
    let mut decals = world
        .query::<(&bevy::prelude::Name, Option<&avian3d::prelude::RigidBody>)>()
        .iter(&world)
        .filter(|(n, _)| n.as_str() == "decal-testdecal")
        .collect::<Vec<_>>();
    assert_eq!(decals.len(), 1);
    assert!(decals.pop().unwrap().1.is_none(), "decals carry no body");

    // Palette alpha on a P8 decal decoded translucent → Blend.
    let mat = materials.iter().map(|(_, m)| m).find(|m| {
        m.base_color_texture.is_some() && matches!(m.alpha_mode, bevy::prelude::AlphaMode::Blend)
    });
    assert!(mat.is_some(), "decal material blends on authored alpha");
}

#[test]
fn texture_override_prefers_later_mount_through_shared_path() {
    let base = tempfile::tempdir().unwrap();
    let over = tempfile::tempdir().unwrap();
    for d in [&base, &over] {
        std::fs::create_dir_all(d.path().join("texture")).unwrap();
    }
    std::fs::write(
        base.path().join("texture/dev_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    std::fs::write(
        over.path().join("texture/dev_road.png"),
        include_bytes!("../../../examples/mods/checker-override/texture/dev_road.png"),
    )
    .unwrap();

    // Base only → the synthetic base image.
    let mut vfs = Vfs::new();
    vfs.mount_dir(base.path(), 0).unwrap();
    let (base_img, _) = load_image(&vfs, "dev_road").expect("base texture decodes");

    // Same-priority later mount wins; the decoded pixels must differ.
    vfs.mount_dir(over.path(), 0).unwrap();
    let resolved = vfs
        .resolve_preferred("texture/dev_road", &["png", "tex"])
        .expect("override resolves");
    assert!(
        resolved.source.path.starts_with(over.path()),
        "override resolved from {:?}",
        resolved.source.path
    );
    let (mod_img, _) = load_image(&vfs, "dev_road").expect("override decodes");
    assert_ne!(base_img.data, mod_img.data);
}

#[test]
fn numbered_frames_load_as_a_texture_sequence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("texture")).unwrap();
    // Frames 1–2 and 4: the sequence stops at the first gap.
    for i in [1, 2, 4] {
        std::fs::write(
            dir.path().join(format!("texture/s_water-{i:04}.png")),
            include_bytes!("../../../assets/texture/dev_road.png"),
        )
        .unwrap();
    }
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    assert!(load_image(&vfs, "s_water").is_none());
    assert_eq!(load_image_sequence(&vfs, "s_water").len(), 2);
    assert!(load_image_sequence(&vfs, "s_missing").is_empty());
}

/// F18-A.6 helper: run `load_city` on `root`'s `city/test.psdl` and
/// return the bound record — the VFS path the session uses.
fn load_test_city(root: &std::path::Path) -> mm2_app::city::LoadedCity {
    let mut vfs = Vfs::new();
    vfs.mount_dir(root, 0).unwrap();
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);
    loaded
}

/// F18-A.6: the sibling `city/<stem>.water` record binds over the
/// loaded PSDL — refs are 1-based room ids, a ref past the room count
/// is skipped rather than reinterpreted, and the record rides
/// `LoadedCity` into the session.
#[test]
fn vfs_to_city_binds_the_water_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), synthetic_psdl()).unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    std::fs::write(root.join("city/test.water"), "0.5\n1\n7\n").unwrap();

    let loaded = load_test_city(root);
    let water = loaded.water.expect("the .water record bound");
    assert_eq!(water.level(), 0.5);
    assert_eq!(water.room_ids().collect::<Vec<_>>(), vec![1]);
    assert_eq!(water.skipped(), 1, "ref 7 resolves to no room");
}

/// Missing and unusable `.water` files never sink a loadable city —
/// the session simply gets no `CityWater` and the wheel-`drag`
/// classification (F05-B.5) still covers water materials.
#[test]
fn vfs_to_city_loads_without_a_water_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), synthetic_psdl()).unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    assert!(load_test_city(root).water.is_none(), "no .water file");

    // A malformed record parses to nothing rather than guessing.
    std::fs::write(root.join("city/test.water"), "deep\n").unwrap();
    assert!(load_test_city(root).water.is_none(), "unparseable level");

    // A non-finite level is rejected even when it parses.
    std::fs::write(root.join("city/test.water"), "NaN\n1\n").unwrap();
    assert!(load_test_city(root).water.is_none(), "non-finite level");
}

/// A record whose refs all fail to resolve binds nothing — the
/// documented failure policy treats it like a missing record rather
/// than inserting a `CityWater` with zero deadly rooms.
#[test]
fn vfs_to_city_rejects_a_water_record_with_no_resolvable_refs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("city")).unwrap();
    std::fs::create_dir_all(root.join("texture")).unwrap();
    std::fs::write(root.join("city/test.psdl"), synthetic_psdl()).unwrap();
    std::fs::write(
        root.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();

    // 0, past-the-end and negative refs all resolve to no room.
    std::fs::write(root.join("city/test.water"), "0.5\n0\n7\n-2\n").unwrap();
    assert!(
        load_test_city(root).water.is_none(),
        "all refs unresolvable -> no resource"
    );

    // A record authoring no refs at all is the same empty binding.
    std::fs::write(root.join("city/test.water"), "0.5\n").unwrap();
    assert!(
        load_test_city(root).water.is_none(),
        "no refs -> no resource"
    );
}

/// Real-content validation: runs only when the gitignored `retail/` tree is
/// present (user-supplied MM2 data). Skips silently otherwise.
#[test]
fn retail_london_imports_driveable_geometry() {
    let retail = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../retail");
    let core = retail.join("mm2core.ar");
    if !core.is_file() {
        eprintln!("retail data not present; skipping London validation");
        return;
    }
    let mut vfs = Vfs::new();
    vfs.mount_archive(&core, 0).expect("mm2core.ar mounts");
    for city in ["london", "sf"] {
        let logical = format!("city/{city}.psdl");
        let Ok((bytes, resolved)) = vfs.read_path(&logical) else {
            eprintln!("{logical} not in archive; skipping");
            continue;
        };
        assert!(resolved.source.kind == mm2_assets::SourceKind::Archive);
        let psdl = Psdl::parse(&bytes).unwrap_or_else(|e| panic!("{logical} parses: {e}"));
        let import = emit_psdl(&psdl, None);
        println!("{city}: {}", import.report);
        println!("{city} spawn: {:?}", import.spawn);

        // TextureRef attributes are consumed by the importer state machine,
        // so they are neither emitted nor rejected; the meaningful failure
        // signals are rejected/malformed refs and unparsed words.
        assert_eq!(import.report.rejected, 0, "malformed references in {city}");
        assert_eq!(import.report.unparsed_words, 0);
        // All known attribute families emit; nothing is unhandled.
        assert!(import.report.unsupported.is_empty());
        // Tunnel walls land on the following road attributes' outer
        // chains — London's underpass railings exercise this path.
        assert!(
            import.report.approximated > 0,
            "tunnel detail flags in {city}"
        );
        assert!(import.colliders.len() > 500, "colliders per room in {city}");
        assert!(import.spawn.is_finite());
        // Spawn must sit near a road surface: within the city bounds plus
        // the spawn clearance, not at bounds centre + 10.
        assert!(
            import.spawn.y < psdl.bounds_max[1] + 5.0,
            "{city} spawn {:?} vs bounds {}",
            import.spawn,
            psdl.bounds_max[1]
        );
    }
}

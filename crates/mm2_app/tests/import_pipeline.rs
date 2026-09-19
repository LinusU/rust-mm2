//! Synthetic end-to-end import tests: VFS bytes → parse → meshes, colliders,
//! materials, props — with no MM2 installation required.

use bevy::asset::Assets;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::Mesh;
use bevy::pbr::StandardMaterial;
use mm2_app::city::{emit_psdl, load_city, load_image, load_image_sequence};
use mm2_assets::Vfs;
use mm2_formats::psdl::Psdl;
use mm2_game::CityEntity;

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

    let import = emit_psdl(&psdl);

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
    // clearance, heading along the road.
    assert!(
        (import.spawn - bevy::math::Vec3::new(0.0, 1.5, -10.0)).length() < 0.01,
        "spawn {:?}",
        import.spawn
    );
    let forward = bevy::math::Quat::from_rotation_y(import.spawn_yaw) * bevy::math::Vec3::NEG_Z;
    assert!(
        forward.x.abs() < 1e-4 && forward.z.abs() > 0.99,
        "{forward:?}"
    );
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
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
        )
        .expect("city loads")
    };
    queue.apply(&mut world);

    // 2 mesh groups + 1 room collider + 1 prop = 4 city entities.
    let city_entities = world.query::<&CityEntity>().iter(&world).count();
    assert_eq!(city_entities, 4);
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
        let import = emit_psdl(&psdl);
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

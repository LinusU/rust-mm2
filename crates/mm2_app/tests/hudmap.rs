//! Integration tests for the F22-A.1 authored HUD minimap.
//!
//! `headless_smoke` is the same runner the `mm2 --headless` binary path
//! uses, so these exercise the production pipeline: a synthetic city
//! carrying a valid `tune/test.mmhudmap` spec plus `PKG3` tile/marker
//! packages must bind the session map (`map=` field present with real
//! tile/marker counts), and a city without the authored content must
//! say `absent` — never a substitute map.

use std::path::Path;

use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{DevOverrides, SessionConfig, WorldMode};
use mm2_vehicle::VehicleConfig;

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

/// A minimal valid `PKG3`: one `H` geometry chunk holding one triangle
/// strip over `verts`/`indices` (fvf = XYZ only — positions alone), plus
/// a float-shader chunk with `paints` paint jobs of one untextured
/// shader each. Enough for `pkg_paint_parts` + `shader_material` to
/// produce real parts.
fn synthetic_pkg(verts: &[[f32; 3]], indices: &[u16], paints: u32) -> Vec<u8> {
    let fvf: u32 = 0x002; // FVF_XYZ
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // n_sections
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    geo.extend_from_slice(&0u32.to_le_bytes()); // sections_duplicate
    geo.extend_from_slice(&fvf.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes()); // n_strips
    geo.extend_from_slice(&0u16.to_le_bytes()); // flags
    geo.extend_from_slice(&0i32.to_le_bytes()); // shader_offset
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim_type = triangles
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut geo, v);
    }
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for i in indices {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut shaders = Vec::new();
    shaders.extend_from_slice(&paints.to_le_bytes()); // shader_type: float, N paints
    shaders.extend_from_slice(&1u32.to_le_bytes()); // shaders per paint job
    for _ in 0..paints {
        push_lp(&mut shaders, ""); // untextured — the diffuse tint carries it
        push_f32s(&mut shaders, &[0.9, 0.8, 0.2, 1.0]); // diffuse
        push_f32s(&mut shaders, &[1.0, 1.0, 1.0, 1.0]); // ambient
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]); // specular
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]); // emissive
        push_f32s(&mut shaders, &[0.0]); // shininess
    }

    let mut pkg = Vec::new();
    pkg.extend_from_slice(b"PKG3");
    for (name, payload) in [("H", geo), ("shaders", shaders)] {
        pkg.extend_from_slice(b"FILE");
        push_lp(&mut pkg, name);
        pkg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        pkg.extend_from_slice(&payload);
    }
    pkg
}

/// The minimal road PSDL — the same shape `smoke.rs`'s
/// `descending_psdl` writes, kept local so this test stands alone.
fn minimal_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes());
    let verts: &[[f32; 3]] = &[
        [-5., 0., 0.],
        [-3., 0., 0.],
        [3., 0., 0.],
        [5., 0., 0.],
        [-5., 0., 20.],
        [-3., 0., 20.],
        [3., 0., 20.],
        [5., 0., 20.],
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    d.extend_from_slice(&1u32.to_le_bytes());
    push_f32s(&mut d, &[0.15]);
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let attr_words: Vec<u16> = vec![0x0a << 3, 1, 0x00, 2, 0, 1, 2, 3, 4, 5, 6, 7];
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes());
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes());
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]);
    d.extend_from_slice(&[0u8; 2]);
    push_f32s(&mut d, &[-5., -60., 0.]);
    push_f32s(&mut d, &[5., 6., 20.]);
    push_f32s(&mut d, &[0., -27., 10.]);
    push_f32s(&mut d, &[70.]);
    d.extend_from_slice(&0u32.to_le_bytes());
    d
}

/// The `mmHudMap` block in the authored shape — distinct values
/// (`ZoomOutDist 700`) so the record proves it read this file, not a
/// default.
const TUNE: &str = "type: a\nmmHudMap {\n  Size 0.21 0.25\n  Pos 0.78 0.75\n  ZoomIn 0\n  Approach Rate 1.2\n  ZoomInDist 350\n  ZoomOutDist 700\n  IconScaleMin 10\n  IconScaleMax 20\n  ZoomInDistFS 400\n  ZoomOutDistFS 900\n  IconScaleMinFS 8\n  IconScaleMaxFS 14\n  Ocean Color 0.1 0.5 0.8\n}\n";

/// A synthetic install with a city, its map spec, and all three map
/// packages: a world-space tile quad plus the tri/square markers.
fn mapped_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", minimal_psdl());
    write(
        d,
        "texture/test_road.png",
        include_bytes!("../../../assets/texture/dev_road.png"),
    );
    write(d, "tune/test.mmhudmap", TUNE);
    // A 200 m world-space tile quad centred on the city.
    write(
        d,
        "geometry/hudmap_test.pkg",
        synthetic_pkg(
            &[
                [-100., 0., -100.],
                [100., 0., -100.],
                [100., 0., 100.],
                [-100., 0., 100.],
            ],
            &[0, 2, 1, 0, 3, 2],
            1,
        ),
    );
    // Player/opponent heading triangle (apex −Z, like the authored one).
    write(
        d,
        "geometry/hudmap_tri.pkg",
        synthetic_pkg(
            &[[-8., 0., 14.], [8., 0., 14.], [0., 0., -14.]],
            &[0, 1, 2],
            10,
        ),
    );
    // Gate/finish dot quad — nine paint jobs like the authored palette.
    write(
        d,
        "geometry/hudmap_square.pkg",
        synthetic_pkg(
            &[[-7., 0., -7.], [7., 0., -7.], [7., 0., 7.], [-7., 0., 7.]],
            &[0, 2, 1, 0, 3, 2],
            9,
        ),
    );
    tmp
}

fn city_config(dev: DevOverrides) -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        dev,
        ..SessionConfig::default()
    }
}

/// A city with authored map content binds the session map: the record's
/// `map=` field carries the bound state (`<view>/<orient>/z<zoom>` —
/// zoomed out at the authored 700 from this spec's `ZoomOutDist`) and
/// the spawned tile/marker counts. A cruise session spawns the tiles,
/// the player tri and the map camera; gate dots wait for an event.
#[test]
fn city_session_binds_authored_map() {
    let install = mapped_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(install.path(), 0).unwrap();
    let rec = smoke::headless_smoke(
        &city_config(DevOverrides::default()),
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        120,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "mapped city should pass: {}",
        rec.line()
    );
    let line = rec.line();
    assert!(
        line.contains(" map=inset/north/z700/hudmap_test.pkg/1t/1m"),
        "record reports the bound map's state and spawned parts: {line}"
    );
}

/// `--pause-map` pauses the first `Playing` frame with the full-screen
/// map up — the headless `map=` field reports `fs` instead of rendering
/// a window.
#[test]
fn pause_map_records_the_fullscreen_state() {
    let install = mapped_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(install.path(), 0).unwrap();
    let rec = smoke::headless_smoke(
        &city_config(DevOverrides {
            pause_map: true,
            ..DevOverrides::default()
        }),
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        120,
        smoke::Driver::Parked,
        None,
    );
    let line = rec.line();
    assert!(
        line.contains("phase=paused"),
        "--pause-map should hold the session paused: {line}"
    );
    assert!(
        line.contains(" map=") && line.contains("/fs/"),
        "record reports the full-screen map: {line}"
    );
}

/// A city without the authored tune/pkg gets no substitute map — the
/// report says exactly which piece was absent.
#[test]
fn city_without_map_content_reports_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", minimal_psdl());
    write(
        d,
        "texture/test_road.png",
        include_bytes!("../../../assets/texture/dev_road.png"),
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(d, 0).unwrap();
    let rec = smoke::headless_smoke(
        &city_config(DevOverrides::default()),
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        120,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "the missing map must not sink the session: {}",
        rec.line()
    );
    assert!(
        rec.line().contains(" map=absent:missing-tune"),
        "record names the absent content: {}",
        rec.line()
    );
}

/// The dev world gets no report at all — its records stay bit-identical
/// to the pre-map baseline.
#[test]
fn dev_world_has_no_map_field() {
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        120,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "expected pass, got: {}",
        rec.line()
    );
    assert!(
        !rec.line().contains(" map="),
        "the dev world carries no map field: {}",
        rec.line()
    );
}

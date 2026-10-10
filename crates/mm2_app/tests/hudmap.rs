//! Integration tests for the F22-A.1 authored HUD minimap.
//!
//! `headless_smoke` is the same runner the `mm2 --headless` binary path
//! uses, so these exercise the production pipeline: a synthetic city
//! carrying a valid `tune/test.mmhudmap` spec plus `PKG3` tile/marker
//! packages must bind the session map (`map=` field present with real
//! tile/marker counts), and a city without the authored content must
//! say `absent` — never a substitute map.

use std::path::Path;

use bevy::prelude::*;
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_formats::hudmap::HudMapSpec;
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

// ---------------------------------------------------------------------------
// F22-C: the world → map coordinate contract (AC02's transform half)
// ---------------------------------------------------------------------------

/// Project a world point through the map camera exactly as the frame
/// would: Bevy's own orthographic matrix on the `ScalingMode::Fixed`
/// projection `drive_hud_map` wrote, through the camera's live
/// `GlobalTransform` and viewport. NDC is −1..1 across the inset.
fn map_ndc(app: &App, camera: Entity, world_pos: Vec3) -> Vec3 {
    let world = app.world();
    let cam = world.get::<Camera>(camera).expect("map camera");
    let gt = world
        .get::<GlobalTransform>(camera)
        .expect("map camera transform");
    let mut proj = world
        .get::<Projection>(camera)
        .expect("map projection")
        .clone();
    let vp = cam.viewport.clone().expect("the inset viewport");
    // `ScalingMode::Fixed` ignores the area, but go through the real
    // trait path with the live viewport size so the matrix is the one
    // the frame would compute.
    bevy::camera::CameraProjection::update(
        &mut *proj,
        vp.physical_size.x as f32,
        vp.physical_size.y as f32,
    );
    let clip = proj.get_clip_from_view();
    let view = gt.affine().inverse().transform_point3(world_pos);
    clip.project_point3(view)
}

/// F22-AC02 (transform half): the map is a world-space instrument, so
/// a known world position lands at its true map coordinate — the
/// player marker exactly at the viewport centre, a point `d` metres
/// north/east at `d / view_half_extent` NDC (the authored zoom's
/// metres-per-half-extent, aspect-corrected on the horizontal) — and
/// the `F` orientation toggle rotates that frame about the driver, it
/// does not re-map the world. North-up: world −Z is screen-up, +X is
/// screen-right. Rotating: the car's heading is screen-up, so the
/// same world points project with the driver's yaw applied.
#[test]
fn known_world_positions_project_to_their_map_coordinates() {
    use bevy::camera::ScalingMode;
    use mm2_app::hud::HudVisible;
    use mm2_app::hudmap::{self, HudMapCamera, HudMapMarker, HudMapReport, MarkerRole};
    use mm2_game::{HudMap, Player, PlayerControl, Session};

    let spec = HudMapSpec::parse(TUNE).unwrap();
    let mut app = App::new();
    app.add_plugins(TransformPlugin)
        .insert_resource(Time::<()>::default())
        .insert_resource(Session::new())
        .insert_resource(HudVisible(true))
        .insert_resource(HudMap::new(spec, 0))
        .insert_resource(HudMapReport {
            spec_path: "tune/test.mmhudmap".into(),
            pkg_path: "geometry/hudmap_test.pkg".into(),
            tiles: 1,
            markers: 1,
            absent: None,
            dot_materials: Vec::new(),
            marker_y: 5.0,
        })
        .add_systems(Update, hudmap::drive_hud_map);
    app.world_mut().spawn((
        bevy::window::PrimaryWindow,
        Window {
            resolution: bevy::window::WindowResolution::new(1280, 960),
            ..default()
        },
    ));
    let player = app
        .world_mut()
        .spawn((
            Player {
                id: mm2_game::PlayerId(0),
                control: PlayerControl::Local,
            },
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();
    let camera = app
        .world_mut()
        .spawn((
            HudMapCamera,
            Camera3d::default(),
            Camera {
                order: 1,
                ..default()
            },
            Transform::default(),
            GlobalTransform::default(),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ))
        .id();
    let marker = app
        .world_mut()
        .spawn((
            HudMapMarker {
                role: MarkerRole::Player,
                extent: 1.0,
            },
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            MeshMaterial3d::<StandardMaterial>::default(),
        ))
        .id();
    // Two frames: `drive_hud_map` writes the transforms in `Update`,
    // propagation settles them at the frame's end, and the second
    // frame's driver reads the settled poses like the real app.
    app.update();
    app.update();

    // The player marker is drawn at the viewport centre — the map is
    // player-centred by construction (the camera parks over the car).
    let marker_pos = app.world().get::<Transform>(marker).unwrap().translation;
    let ndc = map_ndc(&app, camera, marker_pos);
    assert!(
        ndc.truncate().length() < 1e-4,
        "the player marker sits at the map centre, got {ndc:?}"
    );

    // Metres → NDC at the authored zoom (E starts zoomed out in the
    // TUNE fixture: ZoomOutDist 700 × INSET_VIEW_SCALE). Read the
    // production projection rect rather than re-deriving it: the
    // vertical extent must be the authored zoom, the horizontal must
    // follow the inset viewport's aspect, and every metre maps through
    // exactly those numbers.
    let half = app.world().resource::<HudMap>().view_half_extent();
    assert!(
        half > 100.0,
        "the 100 m probe stays inside the view: {half}"
    );
    let vp = app
        .world()
        .get::<Camera>(camera)
        .unwrap()
        .viewport
        .clone()
        .unwrap();
    let (fixed_w, fixed_h) = {
        let world = app.world();
        let Projection::Orthographic(o) = world.get::<Projection>(camera).unwrap() else {
            panic!("the map camera is orthographic");
        };
        let ScalingMode::Fixed { width, height } = o.scaling_mode else {
            panic!("drive_hud_map frames the map with ScalingMode::Fixed");
        };
        (width, height)
    };
    assert!(
        (fixed_h - 2.0 * half).abs() < 1e-3,
        "the vertical extent is the authored zoom: {fixed_h} vs {}",
        2.0 * half
    );
    let vp_aspect = vp.physical_size.x as f32 / vp.physical_size.y as f32;
    assert!(
        (fixed_w - fixed_h * vp_aspect).abs() / fixed_w < 1e-2,
        "the horizontal extent follows the inset viewport's aspect: {fixed_w} vs {fixed_h}·{vp_aspect} \
         (the sub-percent gap is the physical viewport's whole-pixel rounding)"
    );

    // North-up leg: world −Z is screen-up, +X is screen-right, and a
    // point `d` metres out maps to `2d/extent` NDC on its axis.
    let north = map_ndc(&app, camera, Vec3::new(0.0, 0.0, -100.0));
    assert!((north.y - 200.0 / fixed_h).abs() < 1e-4, "north: {north:?}");
    assert!(north.x.abs() < 1e-4, "due north is centred: {north:?}");
    let east = map_ndc(&app, camera, Vec3::new(100.0, 0.0, 0.0));
    assert!((east.x - 200.0 / fixed_w).abs() < 1e-4, "east: {east:?}");
    assert!(east.y.abs() < 1e-4, "due east is level: {east:?}");
    let south = map_ndc(&app, camera, Vec3::new(0.0, 0.0, 100.0));
    assert!((south.y + 200.0 / fixed_h).abs() < 1e-4, "south: {south:?}");

    // Rotating leg (F): the frame rotates about the driver — heading
    // east (`Quat::from_rotation_y(-π/2)` faces +X), so the point off
    // the car's nose reads screen-up and north (the driver's left)
    // reads screen-left. Same world points, rotated frame.
    app.world_mut()
        .resource_mut::<HudMap>()
        .toggle_orientation();
    app.world_mut()
        .get_mut::<Transform>(player)
        .unwrap()
        .rotation = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    app.update();
    app.update();
    let along_heading = map_ndc(&app, camera, Vec3::new(100.0, 0.0, 0.0));
    assert!(
        (along_heading.y - 200.0 / fixed_h).abs() < 1e-4,
        "heading is screen-up while rotating: {along_heading:?}"
    );
    assert!(along_heading.x.abs() < 1e-4, "{along_heading:?}");
    let left_of_driver = map_ndc(&app, camera, Vec3::new(0.0, 0.0, -100.0));
    assert!(
        (left_of_driver.x + 200.0 / fixed_w).abs() < 1e-4,
        "north is the eastward driver's left: {left_of_driver:?}"
    );
    // The player marker is still centred — rotation never moves the
    // driver's own dot.
    let marker_pos = app.world().get::<Transform>(marker).unwrap().translation;
    assert!(map_ndc(&app, camera, marker_pos).truncate().length() < 1e-4);
}

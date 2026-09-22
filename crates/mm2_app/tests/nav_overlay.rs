//! F09-B.2 nav-overlay tests: overlay segment classification/lift/
//! direction over a synthetic graph, `load_city_nav` through the real
//! VFS, and the session-scoped `CityNav` lifecycle through the real
//! load/teardown systems — no original data.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::nav_overlay::{CityNav, OverlayClass, hud_summary, load_city_nav, overlay_lines};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_assets::Vfs;
use mm2_formats::bai::{Bai, END_FILL};
use mm2_game::{
    DevOverrides, ImpactEvent, Mm2Vfs, NavGraph, NavOverlay, NavOverrides, Session, SessionConfig,
    SessionPhase, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn push_v3(d: &mut Vec<u8>, v: [f32; 3]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

fn push_f32s(d: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

/// A `CAI1` byte fixture: two roads chained through one intersection.
///
/// - road 0 spans z −30..−4; its `end` joins intersection 0 (slot 0),
///   its `start` is a dead end.
/// - road 1 spans z 4..30; its `start` joins intersection 0 (slot 1),
///   its `end` is a dead end.
///
/// Each side carries one vehicle lane (x ±3.75) and one sidewalk
/// (x ±8.5). The right-side lane travels with section order (+z), the
/// left-side lane against it.
fn bai_bytes() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"CAI1");
    d.extend_from_slice(&1u16.to_le_bytes()); // n_intersections
    d.extend_from_slice(&2u16.to_le_bytes()); // n_roads

    let write_road =
        |d: &mut Vec<u8>, id: u16, z0: f32, z1: f32, end: (u32, u32), start: (u32, u32)| {
            d.extend_from_slice(&id.to_le_bytes());
            d.extend_from_slice(&2u16.to_le_bytes()); // nSections
            d.extend_from_slice(&0u16.to_le_bytes()); // flags
            d.extend_from_slice(&1u16.to_le_bytes()); // nRooms
            d.extend_from_slice(&1u16.to_le_bytes()); // room 1
            d.extend_from_slice(&7.5f32.to_le_bytes()); // half_width
            d.extend_from_slice(&15.0f32.to_le_bytes()); // base_speed
            for side in [1f32, -1f32] {
                for n in [1u16, 0, 0, 1, 0] {
                    // lanes, trams, trains, sidewalks, ambientTypes
                    d.extend_from_slice(&n.to_le_bytes());
                }
                // [curve][section] distance matrix (lane then sidewalk).
                for _ in 0..2 {
                    for s in [0f32, (z1 - z0).abs()] {
                        d.extend_from_slice(&s.to_le_bytes());
                    }
                }
                for e in [5.0f32, 9.5] {
                    d.extend_from_slice(&e.to_le_bytes());
                }
                d.extend_from_slice(&[0xCDu8; 40]); // misc
                for off in [3.75f32 * side, 8.5f32 * side] {
                    for z in [z0, z1] {
                        push_v3(d, [off, 0.0, z]);
                    }
                }
                // No tram/train curves; sidewalk bounds follow.
                for z in [z0, z1] {
                    push_v3(d, [7.5 * side, 0.0, z]);
                }
                for z in [z0, z1] {
                    push_v3(d, [9.5 * side, 0.0, z]);
                }
            }
            for s in [0f32, (z1 - z0).abs()] {
                d.extend_from_slice(&s.to_le_bytes());
            }
            for z in [z0, z1] {
                push_v3(d, [0.0, 0.0, z]); // origins
            }
            for _ in 0..2 {
                push_v3(d, [1.0, 0.0, 0.0]); // x axes
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 1.0, 0.0]); // y axes
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 0.0, 1.0]); // z axes
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 0.0, 1.0]); // tangents
            }
            // The file stores `end` first, then `start`.
            for (intersection, road_index) in [end, start] {
                d.extend_from_slice(&intersection.to_le_bytes());
                d.extend_from_slice(&0xCDCDu16.to_le_bytes()); // fill0
                d.extend_from_slice(&0u16.to_le_bytes()); // vehicle_rule_code
                d.extend_from_slice(&0u16.to_le_bytes()); // unknown1
                d.extend_from_slice(&road_index.to_le_bytes());
                push_v3(d, [0.0; 3]); // traffic-light origin
                push_v3(d, [0.0; 3]); // traffic-light axis
            }
        };
    write_road(&mut d, 0, -30.0, -4.0, (0, 0), (0, END_FILL));
    write_road(&mut d, 1, 4.0, 30.0, (0, END_FILL), (0, 1));

    // Intersection 0 at the origin, listing both roads.
    d.extend_from_slice(&0u16.to_le_bytes()); // id
    d.extend_from_slice(&1u16.to_le_bytes()); // room
    push_v3(&mut d, [0.0, 0.0, 0.0]);
    d.extend_from_slice(&2u16.to_le_bytes()); // n_roads
    for r in [0u32, 1] {
        d.extend_from_slice(&r.to_le_bytes());
    }

    d.extend_from_slice(&0u32.to_le_bytes()); // culling rooms
    d
}

/// The parsed fixture's graph.
fn graph() -> NavGraph {
    NavGraph::build(&Bai::parse(&bai_bytes()).expect("fixture parses")).graph
}

fn nav(graph: NavGraph) -> CityNav {
    CityNav {
        graph,
        issues: Vec::new(),
        overrides: None,
        route: None,
    }
}

/// One-room PSDL: a single counted fan, enough for `load_city` to emit
/// a mesh so the session's city path succeeds.
fn synthetic_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[[10., 0., 0.], [10., 0., 10.], [20., 0., 10.], [20., 0., 0.]];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&1u32.to_le_bytes()); // texture table (count + 1): none
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&6u32.to_le_bytes()); // nAttr words
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // neighbour room
    }
    // Counted fan: two triangles over the four verts.
    for w in [0x06u16 << 3, 2, 0, 1, 2, 3] {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[10., 0., 0.]); // bounds min
    push_f32s(&mut d, &[20., 6., 10.]); // bounds max
    push_f32s(&mut d, &[15., 3., 5.]); // centre
    push_f32s(&mut d, &[10.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn city_config(route: Option<(u16, u16)>) -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        dev: DevOverrides {
            nav_overlay: Some(NavOverlay { route }),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    }
}

// ---------------------------------------------------------------------------
// overlay_lines — segment classification (no renderer needed)
// ---------------------------------------------------------------------------

#[test]
fn lanes_are_classified_by_kind_and_direction_and_lifted() {
    let nav = nav(graph());
    let segs = overlay_lines(&nav);
    let of = |c| segs.iter().filter(move |s| s.class == c).count();

    // Two roads × right (+z) lane = 2 forward; same for backward; four
    // sidewalk curves; one intersection → two marker strokes.
    assert_eq!(of(OverlayClass::LaneForward), 2);
    assert_eq!(of(OverlayClass::LaneBackward), 2);
    assert_eq!(of(OverlayClass::Sidewalk), 4);
    assert_eq!(of(OverlayClass::Intersection), 2);
    assert!(of(OverlayClass::Direction) > 0);

    // Every lane segment floats LIFT above its curve (vertices y=0).
    for s in segs.iter().filter(|s| {
        matches!(
            s.class,
            OverlayClass::LaneForward | OverlayClass::LaneBackward | OverlayClass::Sidewalk
        )
    }) {
        assert!((s.a.y - 0.35).abs() < 1e-4, "a.y={}", s.a.y);
        assert!((s.b.y - 0.35).abs() < 1e-4, "b.y={}", s.b.y);
    }
}

#[test]
fn direction_chevrons_point_along_each_lanes_travel() {
    let segs = overlay_lines(&nav(graph()));
    // Chevrons on the +x lane travel +z: the tip (segment `a`) sits
    // beyond the base (`b`). The −x lane travels −z.
    let chevrons: Vec<_> = segs
        .iter()
        .filter(|s| s.class == OverlayClass::Direction)
        .collect();
    let fwd: Vec<_> = chevrons.iter().filter(|s| s.a.x > 0.0).collect();
    let bwd: Vec<_> = chevrons.iter().filter(|s| s.a.x < 0.0).collect();
    assert!(!fwd.is_empty() && !bwd.is_empty());
    assert!(fwd.iter().all(|s| s.a.z > s.b.z), "fwd: {fwd:?}");
    assert!(bwd.iter().all(|s| s.a.z < s.b.z), "bwd: {bwd:?}");
}

#[test]
fn closed_roads_draw_in_the_closed_class() {
    let mut nav = nav(graph());
    nav.overrides = Some(NavOverrides {
        closed_roads: [0u16].into_iter().collect(),
        ..NavOverrides::default()
    });
    let segs = overlay_lines(&nav);
    // Road 0's vehicle lanes are Closed; road 1's keep their classes.
    let closed: Vec<_> = segs
        .iter()
        .filter(|s| s.class == OverlayClass::Closed)
        .collect();
    assert!(!closed.is_empty());
    assert!(closed.iter().all(|s| s.a.z < 0.0 && s.b.z < 0.0));
    assert!(
        segs.iter()
            .any(|s| s.class == OverlayClass::LaneForward && s.a.z > 0.0)
    );
}

#[test]
fn a_route_probe_highlights_the_routes_lanes() {
    let mut nav = nav(graph());
    nav.route = Some(
        nav.graph
            .route_roads(0, 1, &mm2_game::RouteOptions::default()),
    );
    assert!(nav.route.as_ref().unwrap().is_ok());
    let segs = overlay_lines(&nav);
    let route: Vec<_> = segs
        .iter()
        .filter(|s| s.class == OverlayClass::Route)
        .collect();
    // Both arcs' lanes highlight, floating at the taller route lift.
    assert!(route.iter().any(|s| s.a.z < 0.0));
    assert!(route.iter().any(|s| s.a.z > 0.0));
    assert!(route.iter().all(|s| (s.a.y - 0.9).abs() < 1e-4));
    // Ordinary lane segments stay at the base lift.
    assert!(
        segs.iter()
            .any(|s| s.class == OverlayClass::Sidewalk && (s.a.y - 0.35).abs() < 1e-4)
    );
}

// ---------------------------------------------------------------------------
// load_city_nav — through the real VFS
// ---------------------------------------------------------------------------

#[test]
fn city_nav_loads_graph_overrides_and_route_through_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "city/test.bai", &bai_bytes());
    // Road 44 is out of range — kept verbatim, matching nothing.
    write(
        dir.path(),
        "city/test.aimap",
        b"[Exceptions]\n1\n44 0.00 0\n\n[Speed Limit]\n15\n",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();

    let nav = load_city_nav(&vfs, &city_config(Some((0, 1)))).expect("nav loads");
    let stats = nav.graph.stats();
    assert_eq!(stats.roads, 2);
    let overrides = nav.overrides.as_ref().expect("aimap applied");
    assert!(overrides.is_closed(44));
    assert_eq!(overrides.default_speed_limit, Some(15.0));
    let route = nav.route.as_ref().expect("probe ran");
    let route = route.as_ref().expect("route reachable");
    assert!(route.steps.len() >= 2, "steps={:?}", route.steps);
    assert!(hud_summary(&nav).starts_with("nav "));
}

#[test]
fn city_nav_is_none_without_the_flag_or_a_city_or_a_graph() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "city/test.bai", &bai_bytes());
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();

    // No `dev.nav_overlay` → no resource, city or not.
    let mut config = city_config(Some((0, 1)));
    config.dev.nav_overlay = None;
    assert!(load_city_nav(&vfs, &config).is_none());
    // The dev world never carries a BAI graph.
    let mut dev = SessionConfig::default();
    dev.dev.nav_overlay = Some(NavOverlay::default());
    assert!(load_city_nav(&vfs, &dev).is_none());
    // A missing `.bai` logs and yields no resource — never fatal.
    let mut config = city_config(None);
    config.world = WorldMode::City {
        psdl: "city/nope.psdl".into(),
    };
    assert!(load_city_nav(&vfs, &config).is_none());
}

// ---------------------------------------------------------------------------
// Session lifecycle — the resource lives and dies with the session
// ---------------------------------------------------------------------------

/// The binary's session wiring minus window/input, like tests/session.
fn test_app(config: SessionConfig, vfs: Vfs) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

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
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(CameraMode::Chase)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
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

fn run_until(app: &mut App, max: usize, mut pred: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..max {
        app.update();
        if pred(app) {
            return true;
        }
    }
    false
}

fn phase_is(app: &mut App, phase: SessionPhase) -> bool {
    *app.world().resource::<Session>().phase() == phase
}

/// AC04 support: a city session carrying `dev.nav_overlay` gains the
/// `CityNav` resource on load and loses it on teardown — the overlay's
/// data never leaks into the next session.
#[test]
fn session_loads_and_tears_down_the_nav_resource() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "city/test.psdl", &synthetic_psdl());
    write(dir.path(), "city/test.bai", &bai_bytes());
    write(
        dir.path(),
        "city/test.aimap",
        b"[Exceptions]\n1\n44 0.00 0\n",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();

    let mut app = test_app(city_config(Some((0, 1))), vfs);
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "city session never reached Playing"
    );
    let nav = app
        .world()
        .get_resource::<CityNav>()
        .expect("CityNav loaded with the city");
    assert_eq!(nav.graph.stats().roads, 2);
    assert!(nav.route.as_ref().is_some_and(|r| r.is_ok()));

    // Quit drives the real teardown path: the resource must die with
    // the session, exactly like RaceState.
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert!(
        app.world().get_resource::<CityNav>().is_none(),
        "CityNav leaked past session teardown"
    );
}

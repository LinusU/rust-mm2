//! F22-A.2: the documented opponent indicator (HUD-3/CTL-1 `I`).
//!
//! The unit legs bind an authored `hudmap_tri.pkg` in a synthetic
//! mount, size the pool to a roster, and assert the toggle's phase
//! gating plus the per-frame binding contract — non-local `Player`
//! participants only, despawn/vehicle-drop releases the slot the same
//! update, toggle-off hides every marker while `bound` still reports
//! live demand. The smoke leg runs the real `headless_smoke` pipeline
//! on a synthetic checkpoint event wiring one opponent, so the `ind=`
//! record field carries the production spawn.

use std::path::Path;

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use mm2_app::oppind::{
    OppIndReport, OppIndicator, OpponentIndicators, drive_opponent_indicators, indicator_input,
    spawn_opponent_indicators,
};
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    EventRef, EventTableKind, Player, PlayerControl, Session, SessionConfig, SessionEntity,
    SessionMode, SessionPhase,
};
use mm2_vehicle::{Vehicle, VehicleConfig};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn oppind_app() -> App {
    let mut app = App::new();
    app.init_resource::<Session>()
        .init_resource::<OpponentIndicators>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_systems(Update, (indicator_input, drive_opponent_indicators));
    app
}

fn press_key(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    // `clear()` keeps held keys — release first so the next press is a
    // real `just_pressed` edge.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(key);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
}

fn transition(app: &mut App, to: SessionPhase) {
    app.world_mut()
        .resource_mut::<Session>()
        .transition(to)
        .unwrap();
}

/// One participant the drive system can bind — `Player` + `Vehicle` +
/// a `GlobalTransform` written to `pos` verbatim (this minimal app
/// runs no propagation).
fn participant(app: &mut App, control: PlayerControl, config: VehicleConfig, pos: Vec3) -> Entity {
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    app.world_mut()
        .spawn((
            SessionEntity(1),
            Player { id, control },
            Vehicle { config },
            Transform::from_translation(pos),
            GlobalTransform::from(Transform::from_translation(pos)),
        ))
        .id()
}

fn dev_config() -> VehicleConfig {
    VehicleConfig::default() // chassis 1.85×0.55×4.4, no hull points
}

/// `spawn_opponent_indicators` takes `&mut Commands` plus the three
/// asset stores — `resource_scope` + a `CommandQueue` is the same
/// dance `tests/mirror.rs` uses for `spawn_mirror`.
fn spawn_pool(app: &mut App, vfs: &Vfs, count: usize) {
    let w = app.world_mut();
    w.resource_scope(|w, mut meshes: Mut<Assets<Mesh>>| {
        w.resource_scope(|w, mut images: Mut<Assets<Image>>| {
            w.resource_scope(|w, mut materials: Mut<Assets<StandardMaterial>>| {
                let mut queue = CommandQueue::default();
                let report = {
                    let mut commands = Commands::new(&mut queue, w);
                    spawn_opponent_indicators(
                        &mut commands,
                        vfs,
                        count,
                        &mut meshes,
                        &mut images,
                        &mut materials,
                        SessionEntity(1),
                    )
                };
                queue.apply(w);
                w.insert_resource(report);
            })
        })
    })
}

fn report(app: &App) -> &OppIndReport {
    app.world().resource::<OppIndReport>()
}

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

/// A minimal valid `PKG3` — the same shape `tests/hudmap.rs` builds for
/// the authored markers. `hudmap_tri` needs ≥9 paint jobs for
/// `TRI_PAINT_OPPONENTS` to index.
fn synthetic_pkg(verts: &[[f32; 3]], indices: &[u16], paints: u32) -> Vec<u8> {
    let fvf: u32 = 0x002; // FVF_XYZ
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    geo.extend_from_slice(&0u32.to_le_bytes());
    geo.extend_from_slice(&fvf.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&0i32.to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut geo, v);
    }
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for i in indices {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut shaders = Vec::new();
    shaders.extend_from_slice(&paints.to_le_bytes());
    shaders.extend_from_slice(&1u32.to_le_bytes());
    for _ in 0..paints {
        push_lp(&mut shaders, "");
        push_f32s(&mut shaders, &[0.9, 0.8, 0.2, 1.0]);
        push_f32s(&mut shaders, &[1.0, 1.0, 1.0, 1.0]);
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut shaders, &[0.0]);
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

/// A mount carrying only the authored marker package the pool binds.
fn tri_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "geometry/hudmap_tri.pkg",
        synthetic_pkg(
            &[[-8., 0., 14.], [8., 0., 14.], [0., 0., -14.]],
            &[0, 1, 2],
            10,
        ),
    );
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn visible_markers(app: &mut App) -> Vec<(Entity, Vec3)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Transform, &Visibility), With<OppIndicator>>();
    q.iter(app.world())
        .filter(|(_, _, v)| **v == Visibility::Visible)
        .map(|(e, t, _)| (e, t.translation))
        .collect()
}

// ---------------------------------------------------------------------------
// Toggle gating (HUD-3/CTL-1: `I` in the live phases only)
// ---------------------------------------------------------------------------

#[test]
fn i_toggles_only_in_the_live_phases() {
    let mut app = oppind_app();

    // Menu keeps the key for its own owner.
    press_key(&mut app, KeyCode::KeyI);
    assert!(app.world().resource::<OpponentIndicators>().0);

    // Countdown and Playing toggle.
    transition(&mut app, SessionPhase::Loading);
    transition(&mut app, SessionPhase::Ready);
    transition(&mut app, SessionPhase::Countdown);
    press_key(&mut app, KeyCode::KeyI);
    assert!(!app.world().resource::<OpponentIndicators>().0);
    transition(&mut app, SessionPhase::Playing);
    press_key(&mut app, KeyCode::KeyI);
    assert!(app.world().resource::<OpponentIndicators>().0);

    // Paused and Results keep the key — the same contract
    // `mirror_input` holds for BACKSPACE.
    transition(&mut app, SessionPhase::Paused);
    press_key(&mut app, KeyCode::KeyI);
    assert!(app.world().resource::<OpponentIndicators>().0);
    transition(&mut app, SessionPhase::Playing);
    transition(&mut app, SessionPhase::Results);
    press_key(&mut app, KeyCode::KeyI);
    assert!(app.world().resource::<OpponentIndicators>().0);
}

// ---------------------------------------------------------------------------
// Pool binding: authored content, live participants, stale release
// ---------------------------------------------------------------------------

/// Two opponents on a two-slot pool: both get a marker riding the
/// authored collider roof (per-car height, not one shared offset),
/// the local car never carries one, and despawn/vehicle-drop releases
/// the slot the same update — the indicator can never hover over a
/// stale or invalid participant.
#[test]
fn markers_ride_only_live_opponents() {
    let mut app = oppind_app();
    let tmp = tri_mount();
    let vfs = vfs_of(tmp.path());
    spawn_pool(&mut app, &vfs, 2);
    assert_eq!(report(&app).markers, 2);
    assert!(report(&app).absent.is_none());
    assert_eq!(
        report(&app).smoke_detail(&OpponentIndicators(true)),
        "on/2m/0b"
    );

    // The world view the markers face — picked via WorldCamera3d, so
    // a Camera3d is the forward pick here.
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        Transform::from_xyz(0.0, 30.0, 60.0),
        GlobalTransform::from(Transform::from_xyz(0.0, 30.0, 60.0)),
    ));

    participant(
        &mut app,
        PlayerControl::Local,
        dev_config(),
        Vec3::new(10.0, 0.0, 0.0),
    );
    let tall = VehicleConfig {
        collider_points: Some(vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [1.0, 3.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 3.0, 1.0],
            [1.0, 3.0, 1.0],
        ]),
        ..VehicleConfig::default()
    };
    let opp_a = participant(&mut app, PlayerControl::Ai, dev_config(), Vec3::ZERO);
    let opp_b = participant(&mut app, PlayerControl::Ai, tall, Vec3::new(0.0, 0.0, 20.0));
    app.update();

    let shown = visible_markers(&mut app);
    assert_eq!(shown.len(), 2, "both opponents ride the pool");
    // Per-car heights: the dev chassis roof (0.55/2 + 0.6 clearance)
    // vs the authored hull roof (3.0 + 0.6).
    let heights: Vec<f32> = shown.iter().map(|(_, p)| p.y).collect();
    assert!(
        heights.iter().any(|y| (y - 0.875).abs() < 0.05),
        "chassis-roof height: {heights:?}"
    );
    assert!(
        heights.iter().any(|y| (y - 3.6).abs() < 0.05),
        "collider-roof height: {heights:?}"
    );
    for (_, p) in &shown {
        assert!(
            (p.x - 10.0).abs() > 1.0,
            "the local car must never carry a marker: {p:?}"
        );
    }
    assert_eq!(report(&app).bound, 2);

    // The remote kind binds too — `control != Local` is the whole
    // contract, so an F25 remote driver is an opponent like the AI.
    app.world_mut().get_mut::<Player>(opp_b).unwrap().control = PlayerControl::Remote;
    app.update();
    assert_eq!(visible_markers(&mut app).len(), 2);

    // Despawn one — its slot frees the same update.
    app.world_mut().despawn(opp_b);
    app.update();
    let shown = visible_markers(&mut app);
    assert_eq!(shown.len(), 1);
    assert!((shown[0].1.z).abs() < 0.5, "the survivor keeps its slot");
    assert_eq!(report(&app).bound, 1);

    // Strip the survivor's Vehicle — no longer a valid target.
    app.world_mut().entity_mut(opp_a).remove::<Vehicle>();
    app.update();
    assert!(visible_markers(&mut app).is_empty());
    assert_eq!(report(&app).bound, 0);

    // Toggle off: markers hide, but `bound` still reports live demand.
    app.world_mut().resource_mut::<OpponentIndicators>().0 = false;
    participant(&mut app, PlayerControl::Ai, dev_config(), Vec3::ZERO);
    app.update();
    assert!(visible_markers(&mut app).is_empty());
    assert_eq!(report(&app).bound, 1);

    // Toggle back on — the marker returns without a respawn.
    app.world_mut().resource_mut::<OpponentIndicators>().0 = true;
    app.update();
    assert_eq!(visible_markers(&mut app).len(), 1);
}

/// Missing authored content binds nothing and says why — the record
/// carries `absent:<why>`, never a substitute marker.
#[test]
fn missing_tri_package_reports_absent() {
    let mut app = oppind_app();
    let tmp = tempfile::tempdir().unwrap(); // nothing authored
    let vfs = vfs_of(tmp.path());
    spawn_pool(&mut app, &vfs, 3);
    assert_eq!(report(&app).absent, Some("missing-pkg"));
    assert_eq!(report(&app).markers, 0);
    assert_eq!(
        report(&app).smoke_detail(&OpponentIndicators(true)),
        "absent:missing-pkg"
    );
}

// ---------------------------------------------------------------------------
// Smoke record: the production `headless_smoke` pipeline reports `ind=`
// ---------------------------------------------------------------------------

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// Minimal `vehCarSim` tune — the same shape `tests/opponents.rs`
/// writes, one opponent car (`vpt`).
fn vehcarsim() -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass 1000\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

/// One quad geometry chunk — the same shape `tests/opponents.rs`
/// writes, centred on `c` (wheels authored in place; no `.mtx`, so the
/// importer falls back to geometry centres).
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&(-1i32).to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    for p in [
        [c[0] - hx, c[1] - hy, c[2] - hz],
        [c[0] + hx, c[1] - hy, c[2] + hz],
        [c[0] + hx, c[1] + hy, c[2] - hz],
        [c[0] - hx, c[1] + hy, c[2] + hz],
    ] {
        for v in p {
            geo.extend_from_slice(&v.to_le_bytes());
        }
        for n in [0.0f32, 1.0, 0.0] {
            geo.extend_from_slice(&n.to_le_bytes());
        }
        for uv in [0.0f32, 0.0] {
            geo.extend_from_slice(&uv.to_le_bytes());
        }
    }
    geo.extend_from_slice(&6u32.to_le_bytes());
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

fn car_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    for (name, geo) in [
        ("body_h", quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6)),
        ("whl0_h", quad_geo([0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl1_h", quad_geo([-0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl2_h", quad_geo([0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("whl3_h", quad_geo([-0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
    ] {
        d.extend_from_slice(b"FILE");
        d.push(name.len() as u8 + 1);
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        d.extend_from_slice(&geo);
    }
    d
}

/// ASCII bound box — underside at local ~0 like the stock cars, so the
/// authored collider roof (~0.9) drives the indicator height.
fn car_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

fn opp_file(points: &[[f32; 3]]) -> String {
    let mut s = OPP_HEADER.to_string();
    for p in points {
        s.push_str(&format!("{},{},{},0,0,0,0,0,0\n", p[0], p[1], p[2]));
    }
    s
}

/// A `race/testcity/` checkpoint event wiring one `vpt` opponent plus
/// the authored `hudmap_tri` the indicator pool binds — the smallest
/// install where `ind=` reports a bound live marker.
fn indicator_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,1,0,0.1,0.0,1,50,1,0,0,0,1,0,0.2,0.0,1,40,1\n"),
    );
    write(
        d,
        "race/testcity/race0.aimap",
        "[Opponent]\n1\nvpt race0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}{}",
            waypoint_row(60.0, 140.0),
            waypoint_row(110.0, 140.0),
            waypoint_row(140.0, 140.0),
            waypoint_row(165.0, 140.0),
            waypoint_row(180.0, 140.0),
        ),
    );
    write(
        d,
        "race/testcity/race0-a-0.opp",
        opp_file(&[
            [70.0, 0.0, 140.0],
            [110.0, 0.0, 140.0],
            [140.0, 0.0, 140.0],
            [165.0, 0.0, 140.0],
            [180.0, 0.0, 140.0],
        ]),
    );
    write(d, "tune/vehicle/vpt.vehcarsim", vehcarsim());
    write(d, "geometry/vpt.pkg", car_pkg());
    write(d, "bound/vpt_bound.bnd", car_bnd());
    write(
        d,
        "geometry/hudmap_tri.pkg",
        synthetic_pkg(
            &[[-8., 0., 14.], [8., 0., 14.], [0., 0., -14.]],
            &[0, 1, 2],
            10,
        ),
    );
    tmp
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The event session binds the authored pool through the production
/// pipeline: the `ind=` field reads `on/1m/1b` — toggle on, one pool
/// slot (the authored roster), one live opponent bound.
#[test]
fn event_session_reports_the_bound_pool() {
    let tmp = indicator_install();
    let rec = smoke::headless_smoke(
        &event_config(),
        vfs_of(tmp.path()),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        240,
        smoke::Driver::Parked,
        None,
    );
    let line = rec.line();
    assert_eq!(rec.status, SmokeStatus::Pass, "event smoke: {line}");
    assert!(
        line.contains(" ind=on/1m/1b"),
        "the authored opponent binds the indicator pool: {line}"
    );
}

/// The dev world never runs the event arm — its record carries no
/// `ind=` field at all.
#[test]
fn dev_world_has_no_indicator_field() {
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(rec.status, SmokeStatus::Pass);
    assert!(
        !rec.line().contains(" ind="),
        "no event → no indicator report: {}",
        rec.line()
    );
}

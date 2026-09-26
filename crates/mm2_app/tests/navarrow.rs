//! F22-A.5: the authored navigation arrow (RACE-6 — the `mmArrow`
//! instrument `mmHUD` owns).
//!
//! The unit legs bind the authored `geometry/hudarrow*.pkg` +
//! `s_hudarrow_*` pair in a synthetic mount, assert the rasterized
//! sprite pair (family tile ahead, shared yellow behind — the pkg's
//! two paint jobs), and drive the node off real `RaceState`
//! resources: bearing rotation, the ahead/behind swap, the
//! `Ordered`/stale/`Complete`/no-race releases, and the `H` gate
//! hiding the node while the report keeps recording demand. The
//! smoke legs run the real `headless_smoke` pipeline on a synthetic
//! checkpoint event so the `arr=` record field carries the
//! production spawn.

use std::path::Path;

use avian3d::prelude::{Position, Rotation};
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use mm2_app::hud::HudVisible;
use mm2_app::navarrow::{
    ArrowFacing, NavArrow, NavArrowReport, NavArrowSprites, spawn_nav_arrow, update_nav_arrow,
};
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, ParticipantState, Player,
    PlayerControl, RaceDefinition, RacePhase, RaceProgress, RaceStart, RaceState, Session,
    SessionConfig, SessionEntity, SessionMode, SessionPhase, TargetSelection,
};
use mm2_vehicle::VehicleConfig;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn arrow_app() -> App {
    let mut app = App::new();
    app.init_resource::<Session>()
        .init_resource::<HudVisible>()
        .init_resource::<Assets<Image>>()
        .add_systems(Update, update_nav_arrow);
    app
}

/// A `Session` stood at `phase` — generation 1 after `begin`, the
/// same generation `RaceState::new(def, 1)` targets.
fn session_at(phase: SessionPhase) -> Session {
    use SessionPhase::*;
    let mut s = Session::new();
    if phase == Menu {
        return s;
    }
    s.begin(SessionConfig::default()).unwrap(); // Loading
    let path: &[SessionPhase] = match phase {
        Ready => &[Ready],
        Countdown => &[Ready, Countdown],
        Playing => &[Ready, Countdown, Playing],
        Paused => &[Ready, Countdown, Playing, Paused],
        Results => &[Ready, Countdown, Playing, Results],
        other => panic!("no session_at path to {other:?}"),
    };
    for step in path {
        s.transition(step.clone()).unwrap();
    }
    s
}

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// Any-order definition with one gate at `(gx, gz)` — the arrow's
/// target while the participant races.
fn def_toward(gx: f32, gz: f32) -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(gx, gz)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 360,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

/// Insert a live race at `phase` against the app's session
/// generation — the same resource `load_session_world` inserts.
fn insert_race(app: &mut App, def: RaceDefinition, phase: RacePhase) {
    let generation = app.world().resource::<Session>().generation();
    let mut race = RaceState::new(def, generation);
    race.phase = phase;
    app.world_mut().insert_resource(race);
}

/// Spawn the local participant `update_nav_arrow` reads — facing
/// −Z (identity `Rotation`) at `pos`, already released to `Racing`.
fn spawn_driver(app: &mut App, def: &RaceDefinition, pos: Vec3) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let mut progress = RaceProgress::new(def);
    progress.state = ParticipantState::Racing;
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id: mm2_game::PlayerId(0),
                control: PlayerControl::Local,
            },
            progress,
            TargetSelection::default(),
            Position(pos),
            Rotation::default(),
        ))
        .id()
}

/// `spawn_nav_arrow` takes `&mut Commands` plus the image store —
/// `resource_scope` + a `CommandQueue`, the same dance
/// `tests/racetime.rs`/`tests/oppind.rs` use.
fn spawn_arrow(app: &mut App, vfs: &Vfs, table: EventTableKind) {
    let w = app.world_mut();
    w.resource_scope(|w, mut images: Mut<Assets<Image>>| {
        let mut queue = CommandQueue::default();
        let report = {
            let mut commands = Commands::new(&mut queue, w);
            spawn_nav_arrow(&mut commands, vfs, &mut images, SessionEntity(1), table)
        };
        queue.apply(w);
        w.insert_resource(report);
    });
}

fn report(app: &App) -> &NavArrowReport {
    app.world().resource::<NavArrowReport>()
}

/// The arrow node's `(rotation, visibility, shown sprite)`.
fn arrow(app: &mut App) -> (Rot2, Visibility, Handle<Image>) {
    let mut q = app
        .world_mut()
        .query_filtered::<(&UiTransform, &Visibility, &ImageNode), With<NavArrow>>();
    let (t, v, i) = q.single(app.world()).unwrap();
    (t.rotation, *v, i.image.clone())
}

fn sprites(app: &mut App) -> (Handle<Image>, Handle<Image>) {
    let mut q = app
        .world_mut()
        .query_filtered::<&NavArrowSprites, With<NavArrow>>();
    let s = q.single(app.world()).unwrap();
    (s.ahead.clone(), s.behind.clone())
}

fn sprite_pixels<'a>(app: &'a App, h: &Handle<Image>) -> (u32, &'a [u8]) {
    let img = app
        .world()
        .resource::<Assets<Image>>()
        .get(h)
        .expect("the bound sprite is an Image asset");
    (
        img.texture_descriptor.size.width,
        img.data.as_deref().unwrap(),
    )
}

fn px(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

// ---------------------------------------------------------------------------
// Synthetic authored content
// ---------------------------------------------------------------------------

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A minimal uncompressed 32bpp TGA — top-left origin, solid fill.
fn tga32(w: u16, h: u16, [r, g, b]: [u8; 3]) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2;
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28;
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[b, g, r, 255]); // BGRA
    }
    t
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

/// The smallest faithful `hudarrow` package: one flat XZ triangle
/// (tip −Z, the authored up-at-bearing-0 direction) with the authored
/// `paints`-deep paint-job table — each job one shader naming
/// `arr_<n>`, so paint 0/1 stand in for the retail family
/// colour/shared yellow pair.
fn arrow_pkg(paints: u32) -> Vec<u8> {
    let fvf: u32 = 0x112; // XYZ + NORMAL + 1 uv set
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections
    geo.extend_from_slice(&3u32.to_le_bytes()); // total verts
    geo.extend_from_slice(&3u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&fvf.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes()); // strips
    geo.extend_from_slice(&0u16.to_le_bytes()); // flags
    geo.extend_from_slice(&0i32.to_le_bytes()); // shader_offset
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim_type triangles
    geo.extend_from_slice(&3u32.to_le_bytes()); // n_vertices
    for (p, uv) in [
        ([-0.5f32, 0.0, 0.7], [0.2f32, 0.8]),
        ([0.5, 0.0, 0.7], [0.8, 0.8]),
        ([0.0, 0.0, -1.0], [0.5, 0.05]),
    ] {
        push_f32s(&mut geo, &p);
        push_f32s(&mut geo, &[0.0, -1.0, 0.0]); // normal
        push_f32s(&mut geo, &uv);
    }
    geo.extend_from_slice(&3u32.to_le_bytes()); // n_indices
    for i in [0u16, 1, 2] {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut shaders = Vec::new();
    shaders.extend_from_slice(&paints.to_le_bytes());
    shaders.extend_from_slice(&1u32.to_le_bytes()); // shaders per job
    for n in 0..paints {
        push_lp(&mut shaders, &format!("arr_{n}"));
        push_f32s(&mut shaders, &[1.0; 4]); // diffuse
        push_f32s(&mut shaders, &[1.0; 4]); // ambient
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]); // specular
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]); // emissive
        push_f32s(&mut shaders, &[0.0]); // shininess
    }

    let mut pkg = b"PKG3".to_vec();
    for (name, payload) in [("H", geo), ("shaders", shaders)] {
        pkg.extend_from_slice(b"FILE");
        push_lp(&mut pkg, name);
        pkg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        pkg.extend_from_slice(&payload);
    }
    pkg
}

/// The synthetic install the arrow binds: `geometry/hudarrow01.pkg`
/// plus the `arr_0`/`arr_1` tiles its two paint jobs name.
fn arrow_mount(dir: &Path) {
    write(dir, "geometry/hudarrow01.pkg", arrow_pkg(2));
    write(dir, "texture/arr_0.tga", tga32(8, 8, [0, 200, 60]));
    write(dir, "texture/arr_1.tga", tga32(8, 8, [230, 200, 0]));
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

// ---------------------------------------------------------------------------
// Binding: authored content through the VFS
// ---------------------------------------------------------------------------

/// The authored package binds two sprites — the family's tile and the
/// shared yellow — and the node starts on the ahead sprite, hidden
/// until a live race wants it.
#[test]
fn authored_pair_binds_and_spawns() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    assert_eq!(report(&app).absent, None);
    assert_eq!(report(&app).sprites, 2);
    assert!(
        report(&app).pkg_path.ends_with("hudarrow01.pkg"),
        "checkpoint binds the generic arrow: {}",
        report(&app).pkg_path
    );

    let (ahead, behind) = sprites(&mut app);
    assert_ne!(ahead, behind, "paint jobs 0 and 1 bind as two sprites");
    let (_, vis, shown) = arrow(&mut app);
    assert_eq!(vis, Visibility::Hidden, "idle until a live race");
    assert_eq!(shown, ahead, "the node starts on the family's tile");
    let owner = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&SessionEntity, With<NavArrow>>();
        *q.single(world).unwrap()
    };
    assert_eq!(owner.0, 1, "session teardown owns the node");
}

/// The rasterized sprites are the authored mesh over each paint's
/// authored tile: the triangle's coverage is opaque inside,
/// transparent outside, tip toward the canvas top — and the two
/// paints carry their own tile's colour.
#[test]
fn sprites_rasterize_the_authored_mesh() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    let (ahead, behind) = sprites(&mut app);
    let (w, data) = sprite_pixels(&app, &ahead);
    assert_eq!(w, mm2_app::navarrow::ARROW_CANVAS_PX);
    // Canvas centre is the authored origin; the triangle's −Z tip
    // lands near the canvas top edge, its base corners lower left
    // and right.
    assert_eq!(px(data, w, w / 2, w / 16)[3], 255, "tip pixel opaque");
    assert_eq!(px(data, w, 27, (w * 3) / 4)[3], 255, "base left");
    assert_eq!(px(data, w, 52, (w * 3) / 4)[3], 255, "base right");
    assert_eq!(px(data, w, 0, 0)[3], 0, "canvas corner stays clear");
    assert_eq!(
        px(data, w, w / 2, w - 2)[3],
        0,
        "below the base stays clear"
    );
    assert_eq!(
        px(data, w, w / 2, w / 2)[..3],
        [0, 200, 60],
        "paint 0 samples its authored tile"
    );
    let (_, bdata) = sprite_pixels(&app, &behind);
    assert_eq!(
        px(bdata, w, w / 2, w / 2)[..3],
        [230, 200, 0],
        "paint 1 carries the shared yellow tile"
    );
}

/// Each event family binds its authored package — Blitz the red
/// `hudarrow_blitz01`, Crash Course the violet `hudarrow_cc01`,
/// Checkpoint/Circuit the generic green `hudarrow01`.
#[test]
fn event_family_selects_its_authored_package() {
    for (table, stem) in [
        (EventTableKind::Blitz, "hudarrow_blitz01"),
        (EventTableKind::CrashCourse, "hudarrow_cc01"),
        (EventTableKind::Checkpoint, "hudarrow01"),
        (EventTableKind::Circuit, "hudarrow01"),
    ] {
        let mut app = arrow_app();
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), &format!("geometry/{stem}.pkg"), arrow_pkg(2));
        write(tmp.path(), "texture/arr_0.tga", tga32(8, 8, [0, 0, 255]));
        write(tmp.path(), "texture/arr_1.tga", tga32(8, 8, [255, 0, 0]));
        let vfs = vfs_of(tmp.path());
        spawn_arrow(&mut app, &vfs, table);
        assert_eq!(
            report(&app).pkg_path,
            format!("geometry/{stem}.pkg"),
            "{table:?} selects its authored package"
        );
        assert_eq!(report(&app).absent, None, "{table:?} binds");
    }
}

/// A package with a single paint job still binds — the behind pick
/// reuses the ahead sprite rather than inventing a tile.
#[test]
fn single_paint_package_reuses_the_ahead_sprite() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "geometry/hudarrow01.pkg", arrow_pkg(1));
    write(tmp.path(), "texture/arr_0.tga", tga32(8, 8, [0, 200, 60]));
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);
    assert_eq!(report(&app).absent, None);
    assert_eq!(report(&app).sprites, 1);
    let (ahead, behind) = sprites(&mut app);
    assert_eq!(ahead, behind);
}

/// Missing artwork binds nothing and says why — the record carries
/// `absent:<why>`, never a substitute mesh.
#[test]
fn missing_package_reports_absent() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);
    assert_eq!(report(&app).absent, Some("missing-pkg"));
    assert_eq!(report(&app).smoke_detail(), "absent:missing-pkg");
    let mut q = app.world_mut().query_filtered::<(), With<NavArrow>>();
    assert_eq!(q.iter(app.world()).count(), 0, "no half-bound node");
}

/// A pkg that resolves but cannot parse reports `unparseable-pkg` —
/// resolve-miss and parse-fail are different causes.
#[test]
fn unparseable_package_reports_absent() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "geometry/hudarrow01.pkg", b"not a pkg");
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);
    assert_eq!(report(&app).absent, Some("unparseable-pkg"));
}

/// A pkg whose texture stem never resolves aborts the instrument —
/// no silently untextured arrow.
#[test]
fn missing_texture_reports_absent() {
    let mut app = arrow_app();
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "geometry/hudarrow01.pkg", arrow_pkg(2));
    write(tmp.path(), "texture/arr_0.tga", tga32(8, 8, [0, 200, 60]));
    // `arr_1` never lands.
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);
    assert_eq!(report(&app).absent, Some("missing-texture"));
    let mut q = app.world_mut().query_filtered::<(), With<NavArrow>>();
    assert_eq!(q.iter(app.world()).count(), 0);
}

// ---------------------------------------------------------------------------
// Driving: authoritative race state → the node
// ---------------------------------------------------------------------------

/// A live AnyOrder race drives the node: gate dead ahead → the
/// family's sprite with bearing 0; the report's `facing` records
/// `ahead`.
#[test]
fn live_target_drives_the_ahead_sprite() {
    let mut app = arrow_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    let def = def_toward(0.0, -60.0);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    app.update();

    let (ahead, _) = sprites(&mut app);
    let (rot, vis, shown) = arrow(&mut app);
    assert_eq!(vis, Visibility::Visible);
    assert_eq!(shown, ahead);
    assert!(rot.as_radians().abs() < 1e-3, "dead-ahead gate: up");
    assert_eq!(report(&app).facing, Some(ArrowFacing::Ahead));
    assert_eq!(report(&app).smoke_detail(), "hudarrow01/ahead");
}

/// A gate in the rear half-plane swaps to the authored yellow tile
/// and swings the rotation past 90° (RACE-6's colour flip).
#[test]
fn rear_target_swaps_to_the_behind_sprite() {
    let mut app = arrow_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    let def = def_toward(0.0, 60.0);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    app.update();

    let (_, behind) = sprites(&mut app);
    let (rot, vis, shown) = arrow(&mut app);
    assert_eq!(vis, Visibility::Visible);
    assert_eq!(shown, behind, "rear target → authored yellow tile");
    assert!(rot.as_radians().abs() > std::f32::consts::FRAC_PI_2);
    assert_eq!(report(&app).facing, Some(ArrowFacing::Behind));
}

/// `Complete`, stale-generation and `Ordered` races release the node
/// — the report reads `off`, never a frozen or foreign target.
#[test]
fn released_states_hide_the_arrow() {
    let mut app = arrow_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    let def = def_toward(0.0, -60.0);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Visible);

    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Complete;
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Hidden);
    assert_eq!(report(&app).facing, None);
    assert_eq!(report(&app).smoke_detail(), "hudarrow01/off");

    insert_race(&mut app, def.clone(), RacePhase::Running);
    app.world_mut().resource_mut::<RaceState>().generation = 99;
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Hidden, "a stale race hides");

    let mut ordered = def.clone();
    ordered.rule = CheckpointRule::Ordered;
    insert_race(&mut app, ordered, RacePhase::Running);
    app.update();
    assert_eq!(
        arrow(&mut app).1,
        Visibility::Hidden,
        "HUD-2 scopes the compass to AnyOrder events"
    );
}

/// The `H` gate hides the node with the rest of the HUD layer while
/// the report keeps recording demand — `arr=` still reads live.
#[test]
fn h_gate_hides_the_arrow_but_keeps_reporting() {
    let mut app = arrow_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);

    let def = def_toward(0.0, -60.0);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Visible);

    app.world_mut().resource_mut::<HudVisible>().0 = false;
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Hidden);
    assert_eq!(report(&app).facing, Some(ArrowFacing::Ahead));

    app.world_mut().resource_mut::<HudVisible>().0 = true;
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Visible);
}

/// Cruise has no race — the node stays dark.
#[test]
fn cruise_shows_no_arrow() {
    let mut app = arrow_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = tempfile::tempdir().unwrap();
    arrow_mount(tmp.path());
    let vfs = vfs_of(tmp.path());
    spawn_arrow(&mut app, &vfs, EventTableKind::Checkpoint);
    app.update();
    assert_eq!(arrow(&mut app).1, Visibility::Hidden);
    assert_eq!(report(&app).facing, None);
}

// ---------------------------------------------------------------------------
// Smoke record: the production `headless_smoke` pipeline reports `arr=`
// ---------------------------------------------------------------------------

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// Minimal `vehCarSim` tune — the same shape `tests/racetime.rs`
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

/// One quad geometry chunk — the same shape `tests/racetime.rs`
/// writes for the opponent's car body.
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
        push_f32s(&mut geo, &p);
        push_f32s(&mut geo, &[0.0, 1.0, 0.0]);
        push_f32s(&mut geo, &[0.0, 0.0]);
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
        push_lp(&mut d, name);
        d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        d.extend_from_slice(&geo);
    }
    d
}

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

/// The `race/testcity/` checkpoint event wiring one `vpt` opponent —
/// the event half of the install, shared by the bound and absent
/// legs (same shape as `tests/racetime.rs`'s fixture).
fn write_event_files(d: &Path) {
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
}

/// The event plus the authored arrow content — the smallest install
/// where `arr=` reports a bound live instrument.
fn arrow_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_event_files(tmp.path());
    arrow_mount(tmp.path());
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

/// The event session binds the authored arrow through the production
/// pipeline: the `arr=` field reads `hudarrow01/<facing>` — the
/// package bound and a live target state off the authoritative race.
#[test]
fn event_session_records_the_bound_arrow() {
    let tmp = arrow_install();
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
        line.contains(" arr=hudarrow01/"),
        "the authored package binds the arrow: {line}"
    );
    assert!(
        !line.contains("arr=absent"),
        "a complete install never reports absent: {line}"
    );
    assert!(
        !line.contains("arr=hudarrow01/off"),
        "a live target reports its facing — the smoke app must drive the arrow: {line}"
    );
}

/// An event session on a mount without the authored package reports
/// `absent` honestly — never substitute art.
#[test]
fn event_session_without_the_package_reports_absent() {
    let tmp = tempfile::tempdir().unwrap();
    write_event_files(tmp.path());
    let rec = smoke::headless_smoke(
        &event_config(),
        vfs_of(tmp.path()),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    let line = rec.line();
    assert_eq!(rec.status, SmokeStatus::Pass, "event smoke: {line}");
    assert!(
        line.contains(" arr=absent:missing-pkg"),
        "no authored package → absent, not a substitute: {line}"
    );
}

/// The dev world never runs the event arm — its record carries no
/// `arr=` field at all.
#[test]
fn dev_world_has_no_arrow_field() {
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
        !rec.line().contains(" arr="),
        "no event → no arrow report: {}",
        rec.line()
    );
}

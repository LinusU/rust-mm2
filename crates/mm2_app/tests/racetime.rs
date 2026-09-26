//! F22-A.4: the authored race timer (HUD-2's stopwatch/countdown pair
//! — `mmHUD`'s `mmTimer` instruments).
//!
//! The unit legs bind the authored `digitac_*`/`digi_colon` glyph set
//! in a synthetic mount, assert the `m:ss:hh` slot composition, and
//! drive the row off real `RaceState` resources — countdown arming,
//! count-up vs count-down, `Complete`/stale release, and the `H` gate
//! hiding the row while the report keeps composing. The smoke leg
//! runs the real `headless_smoke` pipeline on a synthetic checkpoint
//! event so the `tmr=` record field carries the production spawn.

use std::path::Path;

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use mm2_app::hud::HudVisible;
use mm2_app::racetime::{
    RaceTimer, RaceTimerGlyph, RaceTimerReport, TIMER_GLYPH_COUNT, TIMER_SLOTS, TimerDigits,
    TimerGlyph, spawn_race_timer, timer_slots, update_race_timer,
};
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, RaceDefinition, RacePhase,
    RaceStart, RaceState, Session, SessionConfig, SessionEntity, SessionMode, SessionPhase,
};
use mm2_vehicle::VehicleConfig;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn timer_app() -> App {
    let mut app = App::new();
    app.init_resource::<Session>()
        .init_resource::<HudVisible>()
        .init_resource::<Assets<Image>>()
        .add_systems(Update, update_race_timer);
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

fn def(time_limit_ticks: Option<u32>) -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, -60.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks,
        params: EventParams::default(),
        countdown_ticks: 360,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

/// Insert a live race at `phase`/`clock` against the app's session
/// generation — the same resource `load_session_world` inserts.
fn insert_race(app: &mut App, phase: RacePhase, clock: u64, time_limit: Option<u32>) {
    let generation = app.world().resource::<Session>().generation();
    let mut race = RaceState::new(def(time_limit), generation);
    race.phase = phase;
    race.clock = clock;
    app.world_mut().insert_resource(race);
}

/// `spawn_race_timer` takes `&mut Commands` plus the image store —
/// `resource_scope` + a `CommandQueue`, the same dance
/// `tests/mirror.rs`/`tests/oppind.rs` use.
fn spawn_timer(app: &mut App, vfs: &Vfs) {
    let w = app.world_mut();
    w.resource_scope(|w, mut images: Mut<Assets<Image>>| {
        let mut queue = CommandQueue::default();
        let report = {
            let mut commands = Commands::new(&mut queue, w);
            spawn_race_timer(&mut commands, vfs, &mut images, SessionEntity(1))
        };
        queue.apply(w);
        w.insert_resource(report);
    });
}

fn report(app: &App) -> &RaceTimerReport {
    app.world().resource::<RaceTimerReport>()
}

fn timer_visible(app: &mut App) -> bool {
    let mut q = app
        .world_mut()
        .query_filtered::<&Visibility, With<RaceTimer>>();
    q.single(app.world())
        .is_ok_and(|v| *v == Visibility::Visible)
}

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A minimal uncompressed 32bpp TGA — top-left origin, opaque fill.
/// `decode_buffer_image` routes it through Bevy's TGA loader like the
/// authored `digitac_*` files.
fn tga32(w: u16, h: u16) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2; // uncompressed true-colour
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28; // 8 alpha bits + top-left origin
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[0, 255, 0, 255]); // BGRA green
    }
    t
}

/// Write the whole authored glyph set under `texture/` — the 22 stems
/// `spawn_race_timer` resolves.
fn write_glyph_set(dir: &Path) {
    for d in 0..10 {
        write(dir, &format!("texture/digitac_{d}.tga"), tga32(41, 56));
        write(dir, &format!("texture/digitac_{d}_half.tga"), tga32(20, 27));
    }
    write(dir, "texture/digi_colon.tga", tga32(13, 56));
    write(dir, "texture/digi_colon_half.tga", tga32(7, 27));
}

/// A mount carrying only the authored glyph set the timer binds.
fn glyph_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_glyph_set(tmp.path());
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

// ---------------------------------------------------------------------------
// Slot composition — the designed `m:ss:hh` layout (DSN-53)
// ---------------------------------------------------------------------------

/// `m:ss:hh` with leading-zero suppression on minutes: `0` shows
/// `0:00:00`, `59.15 s` shows `0:59:15`, `61.5 s` shows `1:01:50`,
/// ten-plus minutes take a second slot, and the row caps at
/// `999:59:99`.
#[test]
fn slots_compose_minutes_seconds_centiseconds() {
    use TimerGlyph::*;
    assert_eq!(
        timer_slots(0),
        [
            Off,
            Off,
            Digit(0),
            Colon,
            Digit(0),
            Digit(0),
            HalfColon,
            HalfDigit(0),
            HalfDigit(0)
        ]
    );
    // 59.15 s
    assert_eq!(
        timer_slots(5915),
        [
            Off,
            Off,
            Digit(0),
            Colon,
            Digit(5),
            Digit(9),
            HalfColon,
            HalfDigit(1),
            HalfDigit(5)
        ]
    );
    // 61.50 s — minute rollover
    assert_eq!(
        timer_slots(6150),
        [
            Off,
            Off,
            Digit(1),
            Colon,
            Digit(0),
            Digit(1),
            HalfColon,
            HalfDigit(5),
            HalfDigit(0)
        ]
    );
    // 10:07.04 — two-digit minutes engage the second slot only
    let s = timer_slots(60704);
    assert_eq!(s[0], Off);
    assert_eq!(s[1], Digit(1));
    assert_eq!(s[2], Digit(0));
    // 123:45.67 — three-digit minutes fill the row
    let s = timer_slots(742567);
    assert_eq!(s[0], Digit(1));
    assert_eq!(s[1], Digit(2));
    assert_eq!(s[2], Digit(3));
    // Cap — anything at/over 999:59.99 pins at the maximum.
    assert_eq!(timer_slots(u64::MAX)[2], Digit(9));
    assert_eq!(timer_slots(999 * 6000 + 5999), timer_slots(u64::MAX));
}

// ---------------------------------------------------------------------------
// Binding: authored content through the VFS
// ---------------------------------------------------------------------------

/// The full authored set binds all 22 glyphs and spawns one row with
/// nine `SessionEntity`-stamped slots.
#[test]
fn full_glyph_set_binds_and_spawns() {
    let mut app = timer_app();
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);

    assert_eq!(report(&app).glyphs, TIMER_GLYPH_COUNT);
    assert_eq!(report(&app).absent, None);
    let mut roots = app
        .world_mut()
        .query_filtered::<(&TimerDigits, &Visibility, &SessionEntity), With<RaceTimer>>();
    let (_, vis, owner) = roots.single(app.world()).unwrap();
    assert_eq!(*vis, Visibility::Hidden, "idle until a live race");
    assert_eq!(owner.0, 1, "session teardown owns the row");
    let mut glyphs = app.world_mut().query_filtered::<(), With<RaceTimerGlyph>>();
    assert_eq!(glyphs.iter(app.world()).count(), TIMER_SLOTS);
}

/// Missing artwork binds nothing and says why — the record carries
/// `absent:<why>`, never a substitute glyph.
#[test]
fn missing_glyphs_report_absent() {
    let mut app = timer_app();
    let tmp = tempfile::tempdir().unwrap();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    assert_eq!(report(&app).absent, Some("missing-glyphs"));
    assert_eq!(report(&app).glyphs, 0);
    assert_eq!(report(&app).smoke_detail(), "absent:missing-glyphs");
    let mut roots = app.world_mut().query_filtered::<(), With<RaceTimer>>();
    assert_eq!(roots.iter(app.world()).count(), 0, "no half-bound row");
}

/// A partial set aborts the whole instrument — the report counts how
/// far the load got and no row exists.
#[test]
fn partial_glyph_set_reports_absent() {
    let mut app = timer_app();
    let tmp = tempfile::tempdir().unwrap();
    // Everything but the last two `digitac_9` variants.
    for d in 0..9 {
        write(
            tmp.path(),
            &format!("texture/digitac_{d}.tga"),
            tga32(41, 56),
        );
        write(
            tmp.path(),
            &format!("texture/digitac_{d}_half.tga"),
            tga32(20, 27),
        );
    }
    write(tmp.path(), "texture/digi_colon.tga", tga32(13, 56));
    write(tmp.path(), "texture/digi_colon_half.tga", tga32(7, 27));
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    assert_eq!(report(&app).absent, Some("missing-glyphs"));
    assert_eq!(report(&app).glyphs, 20);
    let mut roots = app.world_mut().query_filtered::<(), With<RaceTimer>>();
    assert_eq!(roots.iter(app.world()).count(), 0);
}

// ---------------------------------------------------------------------------
// Driving: authoritative race state → the row
// ---------------------------------------------------------------------------

/// An untimed race counts up on the authoritative `RaceState::clock`
/// — 750 ticks at 120 Hz is `0:06:25`, and the slot images are the
/// authored digit handles, not a text fallback.
#[test]
fn untimed_race_counts_up() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    insert_race(&mut app, RacePhase::Running, 750, None);
    app.update();

    assert!(timer_visible(&mut app));
    assert_eq!(report(&app).display.as_deref(), Some("0:06:25"));
    assert_eq!(report(&app).smoke_detail(), "22g/0:06:25");

    // The slot images are the authored handles the row was built
    // with — digit 6 in the seconds-tens slot, 5 in ones, then the
    // half-size 2 and 5.
    let mut roots = app
        .world_mut()
        .query_filtered::<(&TimerDigits, &Children), With<RaceTimer>>();
    let (digits, children) = roots.single(app.world()).unwrap();
    let expect = [
        None, // minute hundreds off
        None, // minute tens off
        Some(digits.full[0].clone()),
        Some(digits.colon.clone()),
        Some(digits.full[0].clone()),
        Some(digits.full[6].clone()),
        Some(digits.colon_half.clone()),
        Some(digits.half[2].clone()),
        Some(digits.half[5].clone()),
    ];
    assert_eq!(children.len(), TIMER_SLOTS);
    for (i, child) in children.iter().enumerate() {
        let node = app.world().get::<ImageNode>(child).unwrap();
        match &expect[i] {
            Some(h) => assert_eq!(&node.image, h, "slot {i}"),
            None => {
                let node = app.world().get::<Node>(child).unwrap();
                assert_eq!(
                    node.display,
                    Display::None,
                    "suppressed slot {i} takes no layout"
                );
            }
        }
    }
}

/// A timed definition counts down on `time_remaining` — a 50 s Blitz
/// limit 10 s in shows `0:40:00`.
#[test]
fn timed_race_counts_down() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    let limit = 50 * mm2_game::RACE_TICK_HZ;
    insert_race(&mut app, RacePhase::Running, 10 * 120, Some(limit));
    app.update();
    assert!(timer_visible(&mut app));
    assert_eq!(report(&app).display.as_deref(), Some("0:40:00"));
}

/// During `Countdown` the instrument is already armed: an untimed
/// definition shows `0:00:00`; a timed one shows the full limit.
#[test]
fn countdown_phase_arms_the_timer() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Countdown));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);

    insert_race(&mut app, RacePhase::Countdown { remaining: 240 }, 0, None);
    app.update();
    assert_eq!(report(&app).display.as_deref(), Some("0:00:00"));

    let limit = 50 * mm2_game::RACE_TICK_HZ;
    insert_race(
        &mut app,
        RacePhase::Countdown { remaining: 240 },
        0,
        Some(limit),
    );
    app.update();
    assert_eq!(report(&app).display.as_deref(), Some("0:50:00"));
}

/// `Complete` and stale races release the row — the display reads
/// `off` rather than a frozen or foreign clock.
#[test]
fn complete_and_stale_races_hide_the_timer() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);

    insert_race(&mut app, RacePhase::Complete, 9000, None);
    app.update();
    assert!(!timer_visible(&mut app));
    assert_eq!(report(&app).display, None);
    assert_eq!(report(&app).smoke_detail(), "22g/off");

    // A stale resource — a restart generation the teardown has not
    // swept yet — never shows.
    insert_race(&mut app, RacePhase::Running, 750, None);
    app.world_mut().resource_mut::<RaceState>().generation = 99;
    app.update();
    assert!(!timer_visible(&mut app));
    assert_eq!(report(&app).display, None);
}

/// No race at all — the cruise case — keeps the row dark.
#[test]
fn cruise_session_shows_no_timer() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    app.update();
    assert!(!timer_visible(&mut app));
    assert_eq!(report(&app).display, None);
}

/// The `H` gate hides the row with the rest of the HUD layer while
/// the report keeps composing — the `tmr=` field still reads the live
/// value like `ind=`'s `bound`.
#[test]
fn h_gate_hides_the_row_but_keeps_composing() {
    let mut app = timer_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_timer(&mut app, &vfs);
    insert_race(&mut app, RacePhase::Running, 750, None);

    app.world_mut().resource_mut::<HudVisible>().0 = false;
    app.update();
    assert!(!timer_visible(&mut app));
    assert_eq!(report(&app).display.as_deref(), Some("0:06:25"));

    app.world_mut().resource_mut::<HudVisible>().0 = true;
    app.update();
    assert!(timer_visible(&mut app));
}

// ---------------------------------------------------------------------------
// Smoke record: the production `headless_smoke` pipeline reports `tmr=`
// ---------------------------------------------------------------------------

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// Minimal `vehCarSim` tune — the same shape `tests/oppind.rs` writes,
/// one opponent car (`vpt`).
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

/// One quad geometry chunk — the same shape `tests/oppind.rs` writes.
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
/// legs.
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

/// The event plus the authored glyph set — the smallest install where
/// `tmr=` reports a bound live timer.
fn timer_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_event_files(tmp.path());
    write_glyph_set(tmp.path());
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

/// The event session binds the authored glyph set through the
/// production pipeline: the `tmr=` field reads `22g/<m:ss:hh>` — the
/// full set bound, and a live composed display off the authoritative
/// race clock.
#[test]
fn event_session_records_the_bound_timer() {
    let tmp = timer_install();
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
        line.contains(" tmr=22g/"),
        "the authored glyph set binds the timer: {line}"
    );
    assert!(
        !line.contains("tmr=absent"),
        "a complete install never reports absent: {line}"
    );
}

/// An event session on a mount without the authored textures reports
/// `absent` honestly — never substitute art.
#[test]
fn event_session_without_glyphs_reports_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    // The event without the glyph set.
    write_event_files(d);
    let rec = smoke::headless_smoke(
        &event_config(),
        vfs_of(d),
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
        line.contains(" tmr=absent:missing-glyphs"),
        "no authored art → absent, not a substitute: {line}"
    );
}

/// The dev world never runs the event arm — its record carries no
/// `tmr=` field at all.
#[test]
fn dev_world_has_no_timer_field() {
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
        !rec.line().contains(" tmr="),
        "no event → no timer report: {}",
        rec.line()
    );
}

//! F21-B.16: a launched Crash Course lesson speaks its instructor.
//! The row goes through the real `load_session_world` (which binds the
//! lesson's cue table to the session's commentary), the lesson plays
//! to its verdict through the production `advance_race` +
//! `drive_lesson` systems, and the audio side is the shared
//! `audio::commentary_systems` chain `main.rs` registers — so the
//! binding, the schedule entry and the pass/fail → Results timing are
//! all exercised together. Synthetic install (a one-room city, a
//! `race/london/` lesson, a self-authored `ccl0` cue table and tone
//! waves); no original data. The city registry (`aud/spchdata/test.csv`)
//! is deliberately absent: the instructor must not depend on it.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::audio::{self, AudioReport, CommentaryVoice, PcmAudio};
use mm2_app::lesson::LessonDriver;
use mm2_app::race;
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    EventRef, EventTableKind, ImpactEvent, LessonPhase, Mm2Vfs, PlayerVehicle, RacePhase,
    RaceStarted, RaceState, ResultLedger, Session, SessionConfig, SessionEntity, SessionMode,
    SessionPhase, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WP_HEADER: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";
/// The lesson table for London crash row 0.
const TABLE: &str = "aud/spchdata/ccl/ccl0.csv";

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn pcm_wav(rate: u32, frames: usize) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let pcm = vec![0x20u8; frames * 2];
    let mut body = Vec::from(&b"WAVE"[..]);
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    body.extend_from_slice(&pcm);
    let mut out = Vec::from(&b"RIFF"[..]);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
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

/// The same one-room synthetic PSDL `import_pipeline`/`banger` stamp —
/// a road (x −5..5, z 0..20) plus a ground fan — copied so this test
/// stays self-contained (each `tests/` file is a crate).
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
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");
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

/// One lesson (crash row 0: a single slalom gate, `time_limit` seconds)
/// in a one-room city, with the instructor's cue table and waves.
fn lesson_install(time_limit: u32) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    write(
        d,
        "texture/test_road.png",
        include_bytes!("../../../assets/texture/dev_road.png"),
    );
    write(
        d,
        "race/london/mmcrashdata.csv",
        format!(
            "{MM_HEADER}\n\
             lesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"
        ),
    );
    write(d, "race/london/crash0.aimap", "[Opponent]\n0\n");
    for suffix in ["data", "data_p"] {
        write(
            d,
            &format!("race/london/crash0{suffix}.csv"),
            format!("{CRASHDATA}slalom,7,1,{time_limit},0.05,0,0,0,0,0,0\n"),
        );
    }
    write(
        d,
        "race/london/slalom.csv",
        format!("{WP_HEADER}0,1,5,90,15,0,0,0,\n0,1,12,-30,16,0,0,0,\n"),
    );
    write(
        d,
        TABLE,
        "Name prefix/type header,end sufix value,sufix add value\n\
         PRERACE header,,\nCCL00INTRO,2,0\n\
         RESULTSPOOR header,,\nCCL00FAIL,2,0\n\
         RESULTSWIN header,,\nCCL00SUCC,2,0\n",
    );
    for (name, count) in [("intro", 2), ("fail", 2), ("succ", 2)] {
        for n in 1..=count {
            write(
                d,
                &format!("aud/aud11/ccl/ccl00{name}{n:02}.11k.wav"),
                pcm_wav(11025, 11025),
            );
        }
    }
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn lesson_config() -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The session wiring `lesson_launch` runs, plus the audio resources
/// and the shared commentary chain.
fn lesson_app(vfs: Vfs) -> App {
    let selected = session::SelectedCar {
        def: None,
        paint: 0,
    };
    let tuned = VehicleConfig::default();
    let mut session = Session::new();
    session.begin(lesson_config()).unwrap();
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
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .add_message::<mm2_app::cnr::CnrEvent>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(tuned))
        .insert_resource(selected)
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
                race::reanchor_teleported_participants,
                race::advance_race,
                mm2_app::lesson::drive_lesson,
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
                race::update_checkpoint_markers,
            ),
        )
        // The very system chain `main.rs` and the smoke run register.
        .add_systems(Update, audio::commentary_systems());
    app.finish();
    app.cleanup();
    app
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

fn generation(app: &App) -> u64 {
    app.world().resource::<Session>().generation()
}

/// Update until `done`, failing the test after `limit` updates.
fn run_until(app: &mut App, limit: usize, what: &str, done: impl Fn(&mut App) -> bool) {
    for _ in 0..limit {
        app.update();
        if done(app) {
            return;
        }
    }
    panic!("never reached: {what} (phase {:?})", phase(app));
}

/// Sorted `(session generation, stem)` of every spawned commentary voice.
fn voices(app: &mut App) -> Vec<(u64, String)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<(&SessionEntity, &CommentaryVoice)>()
        .iter(app.world())
        .map(|(s, c)| (s.0, c.stem.clone()))
        .collect();
    v.sort();
    v
}

fn stems(app: &mut App, generation: u64, prefix: &str) -> Vec<String> {
    voices(app)
        .into_iter()
        .filter(|(g, s)| *g == generation && s.starts_with(prefix))
        .map(|(_, s)| s)
        .collect()
}

/// Launch to the first countdown frame and let the pre-race window open.
fn launched(time_limit: u32) -> (App, tempfile::TempDir) {
    let tmp = lesson_install(time_limit);
    let mut app = lesson_app(vfs_of(tmp.path()));
    run_until(&mut app, 20, "Countdown", |a| {
        phase(a) == SessionPhase::Countdown
    });
    assert!(app.world().get_resource::<LessonDriver>().is_some());
    for _ in 0..10 {
        app.update();
    }
    (app, tmp)
}

fn running(app: &mut App) {
    run_until(app, 2000, "the lesson's leg running", |a| {
        a.world().resource::<RaceState>().phase == RacePhase::Running
    });
}

#[test]
fn a_launched_lesson_opens_with_its_instructor_before_the_race() {
    let (mut app, _tmp) = launched(30);
    let gen_ = generation(&app);
    // The intro is the lesson's own `CCL00INTRO` line, spoken in the
    // pre-race window with no city registry to draw from…
    let intro = stems(&mut app, gen_, "ccl00");
    assert_eq!(intro.len(), 1, "{:?}", voices(&mut app));
    assert!(intro[0].starts_with("ccl00intro"), "{intro:?}");
    // …and no verdict has been spoken for a lesson nobody has played.
    assert!(stems(&mut app, gen_, "ccl00succ").is_empty());
    assert!(stems(&mut app, gen_, "ccl00fail").is_empty());
    assert_eq!(
        app.world().resource::<LessonDriver>().run().phase(),
        LessonPhase::Running { leg: 0 }
    );
}

#[test]
fn a_cleared_lesson_speaks_its_pass_line_once_while_the_results_stand() {
    let (mut app, _tmp) = launched(30);
    let gen_ = generation(&app);
    running(&mut app);
    // Sweep the car through the leg's only gate.
    let gate = app.world().resource::<RaceState>().definition.checkpoints[0].center;
    let car = player(&mut app);
    let from = app.world().get::<Transform>(car).unwrap().translation;
    place(&mut app, car, from);
    app.update();
    place(&mut app, car, gate);
    run_until(&mut app, 10, "Results", |a| {
        phase(a) == SessionPhase::Results
    });
    assert_eq!(
        app.world().resource::<LessonDriver>().run().phase(),
        LessonPhase::Passed
    );

    // The pass line is audible with the results standing — before any
    // teardown — and speaks exactly once however long they stand.
    run_until(&mut app, 600, "the pass line", |a| {
        !stems(a, gen_, "ccl00succ").is_empty()
    });
    assert_eq!(phase(&app), SessionPhase::Results);
    for _ in 0..600 {
        app.update();
    }
    assert_eq!(
        stems(&mut app, gen_, "ccl00succ").len(),
        1,
        "{:?}",
        voices(&mut app)
    );
    assert!(stems(&mut app, gen_, "ccl00fail").is_empty());
    assert_eq!(stems(&mut app, gen_, "ccl00intro").len(), 1);
}

#[test]
fn a_timed_out_lesson_speaks_its_fail_line_and_a_retry_hears_a_fresh_intro() {
    let (mut app, _tmp) = launched(1);
    let first = generation(&app);
    running(&mut app);
    // Let the one-second limit run out.
    run_until(&mut app, 600, "Results", |a| {
        phase(a) == SessionPhase::Results
    });
    assert!(matches!(
        app.world().resource::<LessonDriver>().run().phase(),
        LessonPhase::Failed { .. }
    ));
    run_until(&mut app, 600, "the fail line", |a| {
        !stems(a, first, "ccl00fail").is_empty()
    });
    assert_eq!(phase(&app), SessionPhase::Results);
    for _ in 0..600 {
        app.update();
    }
    assert_eq!(stems(&mut app, first, "ccl00fail").len(), 1);
    assert!(stems(&mut app, first, "ccl00succ").is_empty());

    // Retry is a session restart: the new attempt opens with the intro
    // again, and the old attempt's verdict does not carry over.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    run_until(&mut app, 60, "the retry's Countdown", |a| {
        phase(a) == SessionPhase::Countdown && generation(a) != first
    });
    for _ in 0..10 {
        app.update();
    }
    let second = generation(&app);
    assert_eq!(
        stems(&mut app, second, "ccl00intro").len(),
        1,
        "{:?}",
        voices(&mut app)
    );
    // Nothing of the old attempt's verdict is carried over into the
    // new one's pre-race window…
    assert!(stems(&mut app, second, "ccl00fail").is_empty());
    assert!(stems(&mut app, second, "ccl00succ").is_empty());
    // …and the retry, left to time out again, earns its own single
    // verdict line.
    run_until(&mut app, 1200, "the retry's fail line", |a| {
        !stems(a, second, "ccl00fail").is_empty()
    });
    for _ in 0..600 {
        app.update();
    }
    assert_eq!(stems(&mut app, second, "ccl00fail").len(), 1);
    assert_eq!(stems(&mut app, second, "ccl00intro").len(), 1);
}

fn player(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .expect("player vehicle spawned")
}

fn place(app: &mut App, car: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(car).unwrap() = Position(pos);
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = pos;
}

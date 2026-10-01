//! F18-B.2 precipitation integration: the effective weather selector
//! binds the standalone `tune/<name>.asbirthrule` + `texture/ptx_<name>`
//! through the real `load_session_world` path — the designed
//! selector→rule mapping (DSN-60), the bounded camera-anchored emitter,
//! the cover/contact approximations and the unload lifecycle — over a
//! synthetic one-room city and a self-authored rule.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::precip::{PrecipFx, PrecipReport};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts, precip};
use mm2_assets::Vfs;
use mm2_game::{
    ImpactEvent, Mm2Vfs, PrecipDrop, Precipitation, RaceStarted, ResultLedger, Session,
    SessionEntity, SessionPhase, TimeOfDay, Weather, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A synthetic standalone rule — the retail `rain.asbirthrule` grammar
/// with self-authored values distinct enough to identify in assertions
/// (spew 40/s, life 0.8 s, ±10 m horizontal jitter).
const RAIN_RULE: &str = "type: a\n\
asBirthRule {\n\
  PositionVar 10.000000 0.000000 10.000000\n\
  Velocity 0.000000 -30.000000 0.000000\n\
  VelocityVar 2.000000 3.000000 2.000000\n\
  Life 0.800000\n\
  LifeVar 0.000000\n\
  Mass 0.300000\n\
  MassVar 0.200000\n\
  Radius 0.400000\n\
  RadiusVar 0.100000\n\
  Drag 0.010000\n\
  DragVar 0.010000\n\
  InitialBlast 0\n\
  SpewRate 40.000000\n\
  SpewTimeLimit 0.000000\n\
  Gravity -9.800000\n\
  TexFrameStart 0\n\
  TexFrameEnd 15\n\
  BirthFlags 8\n\
}\n";

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

/// The same one-room synthetic PSDL `environment`/`banger` stamp — a
/// road (x −5..5, z 0..20) plus a ground fan and a y=5 roof — copied so
/// this test stays self-contained (each `tests/` file is a crate).
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

/// A minimal city install: `city/test.psdl` + its road texture, plus
/// whatever `tune/*.asbirthrule`/`texture/ptx_*` the caller writes.
fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    tmp
}

/// The authored pair — the rule plus a stand-in `ptx_rain` (a real PNG
/// through the same decode path; the retail `.tex` stays out of git).
fn write_rain(d: &Path) {
    write(d, "tune/rain.asbirthrule", RAIN_RULE);
    std::fs::write(
        d.join("texture/ptx_rain.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
}

/// A minimal 16-bit mono PCM RIFF/WAVE — the F18-B.3 ambience leg's
/// authored stems (tests/audio.rs carries the same self-contained
/// helper; each `tests/` file is a crate).
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

/// The precipitation ambience's authored stems — `rainexterior`/
/// `raininterior`/`thunder` under the bank's preferred aud22 tier,
/// the exe-verified names `WeatherAudio` resolves.
fn write_rain_audio(d: &Path) {
    for stem in ["rainexterior", "raininterior", "thunder"] {
        write(d, &format!("aud/aud22/{stem}.22k.wav"), pcm_wav(22050, 220));
    }
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn city_config(weather: u8) -> mm2_game::SessionConfig {
    mm2_game::SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        conditions: mm2_game::SessionConditions {
            time_of_day: TimeOfDay::new(1).unwrap(),
            weather: Weather::new(weather).unwrap(),
        },
        ..mm2_game::SessionConfig::default()
    }
}

/// The same session wiring `environment.rs` runs — the real
/// `load_session_world` + lifecycle driver plus the precipitation
/// systems on a minimal headless app.
fn city_app(config: mm2_game::SessionConfig, vfs: Vfs) -> App {
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
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<PrecipReport>()
        .init_resource::<mm2_app::audio::AudioReport>()
        .init_resource::<Assets<mm2_app::audio::PcmAudio>>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (precip::emit_precip, precip::advance_precip).chain(),
                precip::reset_precip_report.run_if(session::unloading),
                // F18-B.3: the precipitation ambience drive — the
                // production `load_session_world` binds `WeatherAudio`
                // off the same effective-weather pick.
                mm2_app::audio::weather_voices.after(session::drive_session),
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

fn playing(app: &mut App) -> bool {
    matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    )
}

fn live_drops(app: &mut App) -> usize {
    app.world_mut()
        .query::<&PrecipDrop>()
        .iter(app.world())
        .count()
}

/// The active world camera's position — the emitter's anchor.
fn camera_focus(app: &mut App) -> Vec3 {
    app.world_mut()
        .query_filtered::<(&Camera, &GlobalTransform), mm2_app::hudmap::WorldCamera3d>()
        .iter(app.world())
        .find(|(c, _)| c.is_active)
        .map(|(_, t)| t.translation())
        .expect("an active world camera")
}

/// A bound rainy session resolves the rule and atlas through the VFS:
/// the report names the bind, the rig carries the authored spec and
/// the fx assets exist.
#[test]
fn rainy_weather_binds_the_authored_rule() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<PrecipReport>();
    assert_eq!(report.bound, Some("rain"));
    assert_eq!(report.absent, None);
    assert!(report.texture, "the ptx_rain stand-in resolved");
    let rig = app.world().resource::<Precipitation>();
    assert_eq!(rig.spec.spew_rate, 40.0);
    assert_eq!(rig.spec.life, 0.8);
    assert_eq!(rig.spec.tex_frame_end, 15);
    assert_eq!(rig.max_live, 40, "40/s × 0.8 s + margin");
    assert!(app.world().get_resource::<PrecipFx>().is_some());
}

/// A non-precipitating selector binds nothing — no fx, no rig, and a
/// report that formats no `ppt=` field.
#[test]
fn dry_weather_binds_no_precipitation() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(2), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<PrecipReport>();
    assert_eq!(report.bound, None);
    assert_eq!(report.absent, None);
    assert!(app.world().get_resource::<PrecipFx>().is_none());
    assert!(app.world().get_resource::<Precipitation>().is_none());
}

/// A rainy selector whose rule does not resolve is an explicit
/// diagnostic — never a silent dry session (F18-AC06).
#[test]
fn missing_rule_reports_the_absence() {
    let tmp = city_install(); // no tune/ at all
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<PrecipReport>();
    assert_eq!(report.bound, Some("rain"), "the selector still names it");
    assert_eq!(report.absent, Some("rule unavailable"));
    assert!(app.world().get_resource::<PrecipFx>().is_none());
    assert!(app.world().get_resource::<Precipitation>().is_none());
}

/// A rule that resolves but does not parse reports the same explicit
/// absence — not a fabricated spec.
#[test]
fn unparseable_rule_reports_the_absence() {
    let tmp = city_install();
    write(
        tmp.path(),
        "tune/rain.asbirthrule",
        "type: a\nvehCarSim {\n  Mass 1.0\n}\n",
    );
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    let report = app.world().resource::<PrecipReport>();
    assert_eq!(report.bound, Some("rain"));
    assert_eq!(report.absent, Some("rule unparseable"));
    assert!(app.world().get_resource::<Precipitation>().is_none());
}

/// The emitter anchors on the active camera: drops appear inside the
/// authored jitter envelope around the focus, stay under the live
/// bound and conserve — every emitted drop is either live, expired or
/// landed (F18 req 2's camera-relative + bounded legs).
#[test]
fn rainy_session_emits_bounded_camera_relative_drops() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));
    let focus = camera_focus(&mut app);

    for _ in 0..90 {
        app.update();
    }
    let (emitted, expired, landed) = {
        let r = app.world().resource::<PrecipReport>();
        (r.emitted, r.expired, r.landed)
    };
    let live = live_drops(&mut app);
    assert!(emitted > 0, "a rainy session emits drops");
    assert!(
        live <= app.world().resource::<Precipitation>().max_live,
        "live {live} exceeds the bound"
    );
    // Camera-relative: every drop sits inside the authored ±10 jitter
    // around the focus plus fall-time velocity drift (velocity_var 2,
    // life 0.8 s).
    let near = app
        .world_mut()
        .query::<&PrecipDrop>()
        .iter(app.world())
        .all(|d| (d.position.x - focus.x).abs() <= 12.0 && (d.position.z - focus.z).abs() <= 12.0);
    assert!(near, "drops drifted outside the emitter envelope");
    // Conservation: covered candidates never spawn, so every emitted
    // drop is live, expired or landed — no third ledger.
    assert_eq!(
        emitted,
        expired + landed + live as u64,
        "emitted == expired + landed + live"
    );
}

/// Drops die on world contact instead of passing through the road —
/// the swept-segment probe lands them (`landed`), and drops over
/// bare ground expire on `Life` (`expired`): both legs of the
/// declared approximation.
#[test]
fn drops_land_on_contact_and_expire_on_life() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));

    for _ in 0..120 {
        app.update();
    }
    let report = app.world().resource::<PrecipReport>();
    assert!(
        report.landed > 0,
        "drops over the road died on contact: {report:?}"
    );
    assert!(
        report.expired > 0,
        "drops over bare ground expired on life: {report:?}"
    );
}

/// The cover probe: a roof over the whole emission envelope shelters
/// every candidate — nothing spawns, and the suppression counts.
#[test]
fn cover_probe_suppresses_sheltered_spawns() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));
    let focus = camera_focus(&mut app);

    // A roof 8 m over the emitter — every ±10 m candidate's upward
    // probe hits inside the 64 m reach.
    app.world_mut().spawn((
        Collider::cuboid(60.0, 0.5, 60.0),
        Transform::from_translation(focus + Vec3::Y * 8.0),
    ));
    for _ in 0..30 {
        app.update();
    }

    let report = app.world().resource::<PrecipReport>();
    assert_eq!(report.emitted, 0, "a roofed emitter emits nothing");
    assert!(report.covered > 0, "the suppression is counted");
    assert_eq!(live_drops(&mut app), 0);
}

/// Teardown takes the drops and the rig with the session; the restart
/// re-binds a fresh rig and fresh counters — no stale drops cross the
/// generation boundary.
#[test]
fn restart_reloads_the_precipitation_rig() {
    let tmp = city_install();
    write_rain(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    app.update();
    assert!(playing(&mut app));
    for _ in 0..30 {
        app.update();
    }
    let first = app.world().resource::<PrecipReport>().emitted;
    assert!(first > 0);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    for _ in 0..60 {
        app.update();
        if playing(&mut app) {
            break;
        }
    }
    assert!(playing(&mut app), "restart never returned to Playing");
    assert_eq!(app.world().resource::<Session>().generation(), 2);

    // The rig re-bound for generation 2 — and any drop that exists is
    // stamped with it.
    assert!(app.world().get_resource::<Precipitation>().is_some());
    let stale = app
        .world_mut()
        .query::<(&PrecipDrop, &SessionEntity)>()
        .iter(app.world())
        .filter(|(_, o)| o.0 != 2)
        .count();
    assert_eq!(stale, 0, "gen-1 drops survived the restart");
}

/// F18-B.3's production binding leg: `load_session_world` inserts the
/// `WeatherAudio` ambience resource off the same effective-weather
/// pick the particle rig reads — the beds resolve through the
/// session WaveBank on the drive system's first pass — and a dry
/// session binds nothing. Teardown removes the resource with the
/// session (the voice entities are `SessionEntity`-stamped).
#[test]
fn rainy_weather_binds_the_authored_ambience() {
    let tmp = city_install();
    write_rain(tmp.path());
    write_rain_audio(tmp.path());
    let mut app = city_app(city_config(3), vfs_of(tmp.path()));
    for _ in 0..10 {
        app.update();
    }
    assert!(playing(&mut app));

    let ambience = app
        .world()
        .get_resource::<mm2_app::audio::WeatherAudio>()
        .expect("a rainy session binds the ambience resource");
    assert_eq!(ambience.name, "rain");
    let report = app.world().resource::<mm2_app::audio::AudioReport>();
    assert_eq!(report.weather, 2, "both authored beds spawned");
    assert_eq!(report.failed, 0);

    // A dry selector on the same install binds nothing — no resource,
    // no voices, no field activity.
    let mut dry = city_app(city_config(0), vfs_of(tmp.path()));
    for _ in 0..10 {
        dry.update();
    }
    assert!(playing(&mut dry));
    assert!(
        dry.world()
            .get_resource::<mm2_app::audio::WeatherAudio>()
            .is_none(),
        "a dry session carries no ambience binding"
    );
    assert_eq!(
        dry.world()
            .resource::<mm2_app::audio::AudioReport>()
            .weather,
        0
    );

    // Teardown takes the binding and the voices with the session.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    for _ in 0..60 {
        app.update();
        if playing(&mut app) {
            break;
        }
    }
    assert!(playing(&mut app), "restart never returned to Playing");
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    // Generation 2 re-bound its own ambience — and every live weather
    // voice carries its stamp.
    assert!(
        app.world()
            .get_resource::<mm2_app::audio::WeatherAudio>()
            .is_some()
    );
    let stale = app
        .world_mut()
        .query::<(&mm2_app::audio::WeatherVoice, &SessionEntity)>()
        .iter(app.world())
        .filter(|(_, o)| o.0 != 2)
        .count();
    assert_eq!(stale, 0, "gen-1 bed voices survived the restart");
}

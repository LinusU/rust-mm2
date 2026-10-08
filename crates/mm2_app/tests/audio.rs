//! F07-A.2 voice-lifecycle integration: horn request → bounded
//! `PcmAudio` voice spawn, authored stem resolution, teardown ownership
//! and no-device operation — driven headlessly through the production
//! `mm2_app::audio` systems on a synthetic install.
//!
//! No `AudioPlugin`/output device runs here: a spawned voice proves the
//! request→resolve→decode→spawn path; `AudioReport.sunk` staying 0 is
//! the honest no-device report. The sink-attach leg is covered by the
//! windowed `--horn` smoke record.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::{Collider, Gravity, LinearVelocity, PhysicsPlugins};
use bevy::audio::{AudioPlayer, PlaybackMode, PlaybackSettings, SpatialListener, Volume};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::audio::{
    self, AmbientEngineVoice, AmbientRig, AudioReport, AudioVoice, CommentaryAudio,
    CommentaryVoice, EngineVoice, GearWatch, HornRequest, ImpactAudio, PcmAudio, Siren, SirenAudio,
    SkidContact, SurfaceAudio, SurfaceContact, SurfaceRig, SurfaceRole, SurfaceVoice, VoiceKind,
    WaveBank, WeatherAudio, WeatherRole, WeatherVoice, decode_wave,
};
use mm2_app::settings::{AudioLevels, GraphicsSettings};
use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::cardata::{AmbientEngine, CarAudio, SirenStep};
use mm2_formats::materials::{MaterialMap, MaterialSet};
use mm2_game::{
    AmbientAudio, AmbientEngineSpec, Banger, BangerDefinition, DevOverrides, ImpactEvent, ImpactId,
    Mm2Vfs, NavRng, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerVehicle, Session,
    SessionAuthority, SessionConditions, SessionConfig, SessionEntity, SessionPhase,
    SirenSampleSpec, SirenSpec, SurfaceMaterial, SurfaceState, SurfaceVariant, TimeOfDay,
    VehicleAudio, Weather, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::vehicle::Vehicle;
use mm2_vehicle::{DriveDirection, RemoteReplica, VehicleConfig, VehicleState, vehicle_bundle};

use crate::support;

/// A minimal 16-bit mono PCM RIFF/WAVE at `rate` with `frames` frames.
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

fn write(root: &Path, logical: &str, bytes: &[u8]) {
    let path = root.join(logical);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The retail tree shape: one stem under both rate directories, plus a
/// malformed file for the failure leg.
fn fixture_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "aud/aud22/horns/testhorn.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud11/testhorn.11k.wav", &pcm_wav(11025, 110));
    write(d, "aud/aud22/broken.22k.wav", b"not a wave at all");
    tmp
}

/// A cardata horn row naming `stem` (parsed through the real grammar).
fn car_audio(stem: &str) -> CarAudio {
    let csv = format!(
        "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\n{stem},0.9,0,1,REV,0.5\nEngine wave name,a,b\nENG,0.1,0.2\n"
    );
    CarAudio::parse(csv.as_bytes()).unwrap()
}

/// The audio slice of the production app: session already `Playing`
/// (the load path needs the whole render/mesh stack; the horn systems
/// only read `is_playing`/`generation`), the mounted fixture VFS, the
/// session `WaveBank`, and the horn-side Update systems
/// `main`/`headless` schedule. One `PlayerVehicle` carries
/// `VehicleAudio` like `load_session_world` stamps it. The engine rig
/// systems are exercised by [`engine_app`] below.
fn horn_app(dir: &Path, dev_horn: bool) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session
        .begin(SessionConfig {
            dev: DevOverrides {
                horn: dev_horn,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_message::<HornRequest>()
        .add_systems(
            Update,
            (
                audio::horn_input,
                audio::dev_horn_once,
                audio::horn_voices,
                audio::count_sinks,
                audio::sync_audio_pause,
            ),
        );
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleAudio {
            spec: car_audio("testhorn"),
        },
        SessionEntity(generation),
    ));
    app.finish();
    app.cleanup();
    app
}

fn report(app: &App) -> (u64, u64, u64, u64) {
    let r = app.world().resource::<AudioReport>();
    (r.horns, r.voices, r.sunk, r.failed)
}

fn voices(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<AudioVoice>>()
        .iter(app.world())
        .count()
}

fn press_enter(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
}

/// `reset_all`, not `clear` — `clear` keeps `pressed`, so the same key
/// would never re-fire `just_pressed` (no InputPlugin runs here, so the
/// input state is managed by hand).
fn release_enter(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

/// One pad-button edge through the first pad's `digital_mut` (bevy's
/// documented gamepad mocking surface) — `press_enter`'s pad twin.
fn pad_press(app: &mut App, button: GamepadButton) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().press(button);
    }
    app.update();
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().reset_all();
    }
}

#[test]
fn enter_press_spawns_the_authored_horn_voice() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    press_enter(&mut app);
    app.update();
    app.update(); // the request may be drained a frame after it is written
    release_enter(&mut app);

    let (horns, v, sunk, failed) = report(&app);
    assert_eq!((horns, v, sunk, failed), (1, 1, 0, 0));
    assert_eq!(voices(&mut app), 1);
    // The voice carries the session stamp teardown owns, the decoded
    // asset and one-shot despawn settings.
    let world = app.world_mut();
    let (voice, player, settings, session_entity) = world
        .query::<(
            &AudioVoice,
            &AudioPlayer<PcmAudio>,
            &PlaybackSettings,
            &SessionEntity,
        )>()
        .iter(world)
        .next()
        .map(|(v, p, s, e)| (v.kind, p.0.clone(), s.mode, e.0))
        .unwrap();
    assert_eq!(voice, VoiceKind::Horn);
    assert!(matches!(settings, PlaybackMode::Despawn));
    assert_eq!(
        session_entity,
        app.world().resource::<Session>().generation()
    );
    // The decoded asset is the authored wave — aud22's 22 050 Hz copy.
    let waves = app.world().resource::<Assets<PcmAudio>>();
    let pcm = waves.get(&player).unwrap();
    assert_eq!(pcm.sample_rate.get(), 22050);
    assert_eq!(pcm.channels.get(), 1);
}

#[test]
fn horn_press_needs_a_playing_session() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    // Back the phase off to Ready — `load_session_world`'s pre-live
    // phase — where a press must not produce a voice.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Unloading)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Menu)
        .unwrap();

    press_enter(&mut app);
    app.update();
    app.update();
    release_enter(&mut app);
    assert_eq!(report(&app).0, 0);
    assert_eq!(voices(&mut app), 0);
}

/// F22-AC06 pad leg (designed `input::pad` map): `LeftThumb` fires the
/// same `HornRequest` Enter owns — one press, one authored voice — and
/// shares Enter's `is_playing` gate.
#[test]
fn pad_left_thumb_fires_the_authored_horn() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    app.world_mut().spawn(Gamepad::default());
    pad_press(&mut app, GamepadButton::LeftThumb);
    app.update(); // the request may be drained a frame after it is written

    let (horns, v, sunk, failed) = report(&app);
    assert_eq!((horns, v, sunk, failed), (1, 1, 0, 0));
    assert_eq!(voices(&mut app), 1);

    // The phase gate is shared: back the session off to Menu and the
    // pad press is inert like the key.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Unloading)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Menu)
        .unwrap();
    pad_press(&mut app, GamepadButton::LeftThumb);
    app.update();
    assert_eq!(report(&app).0, 1, "no second horn outside Playing");
}

#[test]
fn missing_stem_and_malformed_wave_fail_visibly() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    // Point the binding at a stem nothing ships: the resolve fails.
    {
        let mut q = app.world_mut().query::<&mut VehicleAudio>();
        let mut a = q.single_mut(app.world_mut()).unwrap();
        a.spec.horn.name = "nosuchhorn".into();
    }
    app.world_mut().write_message(HornRequest);
    app.update();
    assert_eq!(report(&app), (1, 0, 0, 1));

    // A stem that resolves but cannot decode fails the same way —
    // explicit errors, never a panic or a silent skip.
    {
        let mut q = app.world_mut().query::<&mut VehicleAudio>();
        let mut a = q.single_mut(app.world_mut()).unwrap();
        a.spec.horn.name = "broken".into();
    }
    app.world_mut().write_message(HornRequest);
    app.update();
    assert_eq!(report(&app), (2, 0, 0, 2));
}

#[test]
fn player_without_authored_audio_spawns_nothing() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    let player = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .unwrap();
    app.world_mut().entity_mut(player).remove::<VehicleAudio>();

    app.world_mut().write_message(HornRequest);
    app.update();
    // The press is consumed and counted; nothing resolves — and that
    // is not a failure, the car simply has no authored horn.
    assert_eq!(report(&app), (1, 0, 0, 0));
    assert_eq!(voices(&mut app), 0);
}

#[test]
fn the_voice_bound_drops_excess_presses() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    for _ in 0..10 {
        app.world_mut().write_message(HornRequest);
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.horns, 10);
    assert_eq!(r.voices, 8, "MAX_HORN_VOICES bounds the live one-shots");
    assert_eq!(r.dropped, 2);
    assert_eq!(voices(&mut app), 8);
}

#[test]
fn teardown_despawns_live_voices_with_the_session() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    app.world_mut().write_message(HornRequest);
    app.update();
    assert_eq!(voices(&mut app), 1);

    // The production teardown system — the same `SessionEntity` sweep
    // `Unloading` runs — removes the voice with the rest of the world.
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    assert_eq!(voices(&mut app), 0);
}

#[test]
fn dev_horn_fires_exactly_once() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), true);
    app.update();
    app.update();
    assert_eq!(report(&app), (1, 1, 0, 0));
    assert_eq!(voices(&mut app), 1);
}

#[test]
fn bank_prefers_the_22k_variant() {
    let dir = fixture_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let mut bank = WaveBank::index(&vfs);
    let mut waves = Assets::<PcmAudio>::default();
    let h = bank.load(&vfs, &mut waves, "TESTHORN").unwrap();
    assert_eq!(waves.get(&h).unwrap().sample_rate.get(), 22050);
    // The stem match is case- and suffix-insensitive.
    let h2 = bank.load(&vfs, &mut waves, "testhorn").unwrap();
    assert_eq!(h, h2, "the decode cache dedupes repeated loads");
}

// ---------------------------------------------------------------------------
// F07-B.1: engine loop rig — authored fade-window rows → looping voices
// re-mixed from the sim's RPM.
// ---------------------------------------------------------------------------

/// `vpbug`-shaped values for one canonical fade-window row.
const FADE_VALUES: &str = "0.55,0.835,1,800,2500,7000,0.85,2,1,7000";

/// A cardata table whose engine rows ride the canonical fade-window
/// header; `rows` are `name,values` lines appended verbatim.
fn engine_car_audio(rows: &str) -> CarAudio {
    let csv = format!(
        "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nTESTHORN,0.9,0,4,REV,0.5\nEngine wave name,Min Volume,Max Volume,fade in start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\n{rows}"
    );
    CarAudio::parse(csv.as_bytes()).unwrap()
}

/// The four vpbug-analogue rows plus their waves on the fixture tree.
fn engine_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for stem in ["eidle", "edrive", "emid", "ehigh"] {
        write(
            d,
            &format!("aud/aud22/engines/{stem}.22k.wav"),
            &pcm_wav(22050, 220),
        );
    }
    tmp
}

const ENGINE_ROWS: &str = "EIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\nEDRIVE,0.55,0.9,500,2800,6500,10000,0.6,2.5,500,12000\nEMID,0.55,0.85,500,4000,7000,11000,1,2.25,500,11000\nEHIGH,0.55,0.91,3000,8000,15000,15000,0.65,2.25,3000,12000\n";

/// Like [`horn_app`] but with the engine rig systems and a
/// `VehicleState`-carrying player — the entity `load_session_world`
/// produces for a cardata-backed car.
fn engine_app(dir: &Path, rows: &str) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(Update, (audio::engine_rigs, audio::engine_drive).chain());
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleAudio {
            spec: engine_car_audio(rows),
        },
        VehicleState::new(&VehicleConfig::default()),
        SessionEntity(generation),
        Transform::default(),
    ));
    app.finish();
    app.cleanup();
    app
}

fn engine_voices(app: &mut App) -> Vec<(usize, f32, f32)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<&EngineVoice>()
        .iter(app.world())
        .map(|v| (v.row, v.mix.volume, v.mix.speed))
        .collect();
    v.sort_by_key(|(row, _, _)| *row);
    v
}

fn player(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .unwrap()
}

/// Spawn a second drivable car without `PlayerVehicle` — the entity
/// shape `spawn_opponents` produces for a cardata-backed roster car.
fn spawn_opponent_car(app: &mut App, rows: &str) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .spawn((
            VehicleAudio {
                spec: engine_car_audio(rows),
            },
            VehicleState::new(&VehicleConfig::default()),
            SessionEntity(generation),
            Transform::default(),
        ))
        .id()
}

#[test]
fn engine_rig_spawns_one_loop_voice_per_drivable_row() {
    let dir = engine_dir();
    let mut app = engine_app(dir.path(), ENGINE_ROWS);
    app.update();

    let car = player(&mut app);
    let generation = app.world().resource::<Session>().generation();
    let world = app.world_mut();
    let mut voices = world.query::<(
        &AudioVoice,
        &EngineVoice,
        &AudioPlayer<PcmAudio>,
        &PlaybackSettings,
        &ChildOf,
        &SessionEntity,
    )>();
    let all: Vec<_> = voices.iter(world).collect();
    assert_eq!(all.len(), 4);
    for (voice, _, _, settings, child_of, session_entity) in &all {
        assert_eq!(voice.kind, VoiceKind::Engine);
        assert!(matches!(settings.mode, PlaybackMode::Loop));
        assert_eq!(settings.volume, bevy::audio::Volume::Linear(0.0));
        // The voice is the car's child — it despawns with it — and
        // still carries the session stamp every voice reports.
        assert_eq!(child_of.parent(), car);
        assert_eq!(session_entity.0, generation);
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.loops, 4);
    assert_eq!(r.voices, 4);
    assert_eq!(r.failed, 0);
    // The default-config car idles at 900 rpm: the high loop's band
    // (3000+) is silent, the other three are fading in — the mix ran.
    assert_eq!(r.audible, 3);
    let mixes = engine_voices(&mut app);
    assert_eq!(mixes[3].1, 0.0, "row 3 (high) below its band");
    assert!(mixes[0].1 > 0.8, "idle at max volume");
}

#[test]
fn engine_mix_tracks_the_sim_rpm() {
    let dir = engine_dir();
    let mut app = engine_app(dir.path(), ENGINE_ROWS);
    app.update();
    let idle_mixes = engine_voices(&mut app);

    // Drive the sim state, not the component: the next frame re-mixes
    // every loop off the new RPM through the production system.
    let car = player(&mut app);
    app.world_mut().get_mut::<VehicleState>(car).unwrap().rpm = 6000.0;
    app.update();
    let mixes = engine_voices(&mut app);

    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.audible, 4, "every band overlaps at 6000 rpm");
    for (i, (before, after)) in idle_mixes.iter().zip(&mixes).enumerate() {
        assert!(
            after.2 > before.2,
            "row {i} pitch rises with rpm: {} -> {}",
            before.2,
            after.2
        );
    }
    // The idle loop is deep in fade-out while the high loop has
    // swelled — a crossfade, not a volume shift.
    assert!(mixes[0].1 < idle_mixes[0].1, "idle fading");
    assert!(mixes[3].1 > 0.5, "high loop inside its band");
}

#[test]
fn unusable_rows_and_missing_waves_report_without_sinking_the_rig() {
    let dir = engine_dir();
    // EBAD is short a tail (no fade values to resolve) and EMISS names
    // a stem nothing ships — both count once at rig build while the
    // good rows still spawn.
    let rows = format!("EIDLE,{FADE_VALUES}\nEBAD,0.5,0.9\nEMISS,{FADE_VALUES}\n");
    let mut app = engine_app(dir.path(), &rows);
    app.update();

    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.loops, 1);
    assert_eq!(r.failed, 2);
    assert_eq!(engine_voices(&mut app).len(), 1);

    // The rig marker means the failures report once — a later update
    // must not re-attempt the same rows.
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.loops, 1);
    assert_eq!(r.failed, 2);
}

#[test]
fn the_engine_voice_bound_caps_giant_tables() {
    let dir = engine_dir();
    // Ten authored rows against the one resolved stem — the bound
    // keeps eight and counts the rest refused.
    let rows = (0..10)
        .map(|_| format!("EIDLE,{FADE_VALUES}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = engine_app(dir.path(), &rows);
    app.update();

    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.loops, 8);
    assert_eq!(r.dropped, 2);
    assert_eq!(engine_voices(&mut app).len(), 8);
}

#[test]
fn a_car_without_authored_audio_builds_no_rig() {
    let dir = engine_dir();
    let mut app = engine_app(dir.path(), ENGINE_ROWS);
    let car = player(&mut app);
    app.world_mut().entity_mut(car).remove::<VehicleAudio>();
    app.update();

    assert_eq!(engine_voices(&mut app).len(), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.loops, r.voices, r.failed), (0, 0, 0));
}

// ---------------------------------------------------------------------------
// F07-B.2: opponent rigs are spatial emitters; the player's rig stays
// non-spatial; the listener rides the active 3-D camera.
// ---------------------------------------------------------------------------

#[test]
fn opponent_rigs_are_spatial_and_the_players_is_not() {
    let dir = engine_dir();
    let mut app = engine_app(
        dir.path(),
        "EIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\n",
    );
    spawn_opponent_car(
        &mut app,
        "EMID,0.55,0.85,500,4000,7000,11000,1,2.25,500,11000\n",
    );
    app.update();

    let world = app.world_mut();
    let mut q = world.query_filtered::<(&PlaybackSettings, &ChildOf), With<EngineVoice>>();
    let (mut player_cfg, mut opp_cfg) = (None, None);
    for (settings, child) in q.iter(world) {
        let slot = if world.get::<PlayerVehicle>(child.parent()).is_some() {
            &mut player_cfg
        } else {
            &mut opp_cfg
        };
        *slot = Some((settings.spatial, settings.spatial_scale.is_some()));
    }
    assert_eq!(
        player_cfg,
        Some((false, false)),
        "the local car anchors the mix — no spatial attenuation"
    );
    assert_eq!(
        opp_cfg,
        Some((true, true)),
        "opponent emitters are spatial with the designed scale"
    );
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.rigs, r.loops, r.voices), (2, 2, 2));
    assert_eq!(r.failed, 0);
}

#[test]
fn opponent_loops_mix_off_their_own_rpm() {
    let dir = engine_dir();
    // Both cars carry only the high-band row: audible at 6000 rpm,
    // silent at the 900 rpm idle.
    let row = "EHIGH,0.55,0.91,3000,8000,15000,15000,0.65,2.25,3000,12000\n";
    let mut app = engine_app(dir.path(), row);
    let opponent = spawn_opponent_car(&mut app, row);
    app.update();

    let mut voices = app.world_mut().query::<(&EngineVoice, &ChildOf)>();
    let mixes: Vec<bool> = voices
        .iter(app.world())
        .map(|(v, c)| (world_is_player(app.world(), c.parent()), v.mix.volume > 0.0))
        .map(|(_, audible)| audible)
        .collect();
    assert_eq!(mixes, [false, false], "both cars idle below the band");

    app.world_mut()
        .get_mut::<VehicleState>(opponent)
        .unwrap()
        .rpm = 6000.0;
    app.update();

    let mut voices = app.world_mut().query::<(&EngineVoice, &ChildOf)>();
    let mut seen = (false, false);
    for (v, c) in voices.iter(app.world()) {
        if world_is_player(app.world(), c.parent()) {
            seen.0 = v.mix.volume > 0.0;
        } else {
            seen.1 = v.mix.volume > 0.0;
        }
    }
    assert_eq!(
        seen,
        (false, true),
        "the opponent's band opened off its own rpm while the player idles"
    );
    assert_eq!(app.world().resource::<AudioReport>().audible, 1);
}

fn world_is_player(world: &World, entity: Entity) -> bool {
    world.get::<PlayerVehicle>(entity).is_some()
}

#[test]
fn the_rig_bound_caps_silent_cars() {
    let dir = engine_dir();
    let mut app = engine_app(
        dir.path(),
        "EIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\n",
    );
    // The player rig plus twenty opponents: sixteen rigs build, five
    // cars report the bound and stay silent — once, not every frame.
    for _ in 0..20 {
        spawn_opponent_car(&mut app, "EIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\n");
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.rigs, 16);
    assert_eq!(r.dropped, 5);
    assert_eq!(r.loops, 16);

    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(
        (r.rigs, r.dropped, r.loops),
        (16, 5, 16),
        "no re-warn churn"
    );
}

#[test]
fn the_listener_follows_the_active_3d_camera() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_systems(Update, audio::audio_listener);
    let chase = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
        ))
        .id();
    let free = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: false,
                ..default()
            },
        ))
        .id();

    app.update();
    assert!(
        app.world().get::<SpatialListener>(chase).is_some(),
        "the active camera is the ear"
    );
    assert!(app.world().get::<SpatialListener>(free).is_none());

    // A mode switch (the `C` toggle's effect on `Camera::is_active`)
    // moves the listener the same frame rather than doubling it.
    app.world_mut().get_mut::<Camera>(chase).unwrap().is_active = false;
    app.world_mut().get_mut::<Camera>(free).unwrap().is_active = true;
    app.update();
    assert!(app.world().get::<SpatialListener>(chase).is_none());
    assert!(app.world().get::<SpatialListener>(free).is_some());
}

#[test]
fn teardown_despawns_engine_voices_with_the_car() {
    let dir = engine_dir();
    let mut app = engine_app(dir.path(), ENGINE_ROWS);
    app.update();
    assert_eq!(engine_voices(&mut app).len(), 4);

    // The production teardown sweeps session roots; the voices are the
    // car's children and cascade with it (F07-AC06's restart leg).
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(engine_voices(&mut app).len(), 0);
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<PlayerVehicle>>()
            .iter(app.world())
            .count(),
        0,
        "the car itself is gone"
    );
}

#[test]
fn decode_wave_reports_unsupported_and_unbounded() {
    // Structural garbage.
    assert!(decode_wave(b"not a wave").is_err());
    // A non-PCM tag reports, it does not decode.
    let mut adpcm = pcm_wav(22050, 64);
    adpcm[20] = 0x11; // fmt tag: IMA ADPCM
    let err = decode_wave(&adpcm).unwrap_err();
    assert!(err.contains("format tag"), "{err}");
    // The bound refuses a giant-but-valid file before allocating.
    let huge = pcm_wav(22050, 9 * 1024 * 1024);
    let err = decode_wave(&huge).unwrap_err();
    assert!(err.contains("bound"), "{err}");
    // Degenerate rates/channels error rather than div-by-zero later.
    let zero = pcm_wav(0, 4);
    assert!(decode_wave(&zero).is_err());
}

// ---------------------------------------------------------------------------
// F07-B.3: deduplicated impacts → bounded one-shot voices. The session
// `ImpactAudio` holds the authored `default_impacts.csv`; the struck
// side's `AudioId` selects the category, `severity × striker mass`
// picks the force band.
// ---------------------------------------------------------------------------

/// A fixture install: three decodable waves at distinguishing rates
/// plus the authored impact table that names them.
fn impact_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "aud/aud22/impacts/soft.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud22/impacts/huge.22k.wav", &pcm_wav(48000, 220));
    write(d, "aud/aud22/impacts/prop.22k.wav", &pcm_wav(11025, 110));
    write(
        d,
        "aud/cardata/player/default_impacts.csv",
        b"***\nBanger name,Num samples,ID\nWALL,2,0\nsample name,min volume,max volume,min force,max force,frequency\nSOFT,0.5,0.6,1000,8000,1.0\nHUGE,0.9,1.0,8000,999999,1.0\n***\nBanger name,Num samples,ID\nLIGHT,1,7\nsample name,min volume,max volume,min force,max force,frequency\nPROP,0.4,0.4,0,999999,1.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n",
    );
    tmp
}

/// The audio slice of the production app for impact voices: a `Playing`
/// session, the mounted fixture VFS, the session `WaveBank` +
/// `ImpactAudio` (loaded through the production path), and the
/// `impact_voices` system on Update like the live schedules wire it.
fn impact_app(dir: &Path) -> App {
    impact_app_with(dir, SessionAuthority::Local)
}

/// `impact_app` under an explicit session authority — the F25-B v10
/// legs need a predicted (`Remote`) session to cover the replicated
/// stream's split from the local one.
fn impact_app_with(dir: &Path, authority: SessionAuthority) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session
        .begin(SessionConfig {
            authority,
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let generation = session.generation();
    // Production inserts the resource only when the authored table
    // loads — an absent record leaves none and degrades to silence.
    let table = ImpactAudio::load(&vfs, generation);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_message::<ImpactEvent>()
        .add_message::<mm2_app::netdrive::RemoteImpact>()
        .add_systems(Update, audio::impact_voices);
    if let Some(table) = table {
        app.insert_resource(table);
    }
    app.finish();
    app.cleanup();
    app
}

/// Mint a session object id off the live allocator so participants
/// read as session objects, then attach `bundle` to its entity.
fn spawn_object(app: &mut App, bundle: impl Bundle) -> (ObjectId, Entity) {
    let id = app.world_mut().resource_mut::<Session>().mint_object_id();
    let entity = app.world_mut().spawn((ObjectIdentity(id), bundle)).id();
    (id, entity)
}

/// A drivable car like `load_session_world`/`spawn_opponents` stamps:
/// `Vehicle` + `Mass` through the bundle, the session identity and the
/// driver role the spatial rule reads.
fn spawn_test_car(app: &mut App, control: PlayerControl, mass: f32) -> (ObjectId, Entity) {
    let config = VehicleConfig {
        mass,
        ..VehicleConfig::default()
    };
    let player_id = app.world_mut().resource_mut::<Session>().mint_player_id();
    spawn_object(
        app,
        (
            Player {
                id: player_id,
                control,
            },
            vehicle_bundle(&config),
        ),
    )
}

fn write_impact(app: &mut App, a: ObjectId, b: ObjectId, severity: f32) {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut().write_message(ImpactEvent {
        id: ImpactId(1),
        generation,
        tick: 0,
        participants: (a, b),
        point: Vec3::new(1.0, 2.0, 3.0),
        normal: Vec3::Y,
        severity,
        surface: SurfaceState::default(),
    });
}

/// `(kind, sample_rate, spatial)` for every live voice.
fn impact_voices(app: &mut App) -> Vec<(VoiceKind, u32, bool)> {
    let world = app.world_mut();
    let mut q = world.query::<(&AudioVoice, &AudioPlayer<PcmAudio>, &PlaybackSettings)>();
    let mut out: Vec<_> = q
        .iter(world)
        .map(|(v, p, s)| {
            let rate = world
                .resource::<Assets<PcmAudio>>()
                .get(&p.0)
                .unwrap()
                .sample_rate
                .get();
            (v.kind, rate, s.spatial)
        })
        .collect();
    out.sort_by_key(|(_, rate, _)| *rate);
    out
}

#[test]
fn a_wall_impact_picks_the_authored_band() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    // 1300 kg default-mass car (VehicleConfig::default): 3 m/s →
    // force 3900 lands SOFT's 1000–8000 band.
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    write_impact(&mut app, car, ObjectId::WORLD, 3.0);
    app.update();

    let voices = impact_voices(&mut app);
    assert_eq!(voices, [(VoiceKind::Impact, 22050, false)]);
    let world = app.world_mut();
    let (_, settings, transform) = world
        .query::<(&AudioVoice, &PlaybackSettings, &Transform)>()
        .iter(world)
        .next()
        .map(|(v, s, t)| (v.kind, *s, *t))
        .unwrap();
    assert!(matches!(settings.mode, PlaybackMode::Despawn));
    assert!(!settings.spatial, "the local player's hits anchor the mix");
    let volume = settings.volume.to_linear();
    assert!(
        (0.5..=0.6).contains(&volume),
        "volume drawn inside the authored range: {volume}"
    );
    assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.impacts, r.voices, r.failed), (1, 1, 0));

    // 30 m/s → 39 000 lands HUGE's band — a different authored sample.
    write_impact(&mut app, car, ObjectId::WORLD, 30.0);
    app.update();
    let voices = impact_voices(&mut app);
    assert!(
        voices.contains(&(VoiceKind::Impact, 48000, false)),
        "the huge band resolves its own sample: {voices:?}"
    );
}

#[test]
fn a_sub_floor_touch_is_authored_silent() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    // 0.5 m/s × 1300 kg = 650 — below SOFT's authored 1000 floor.
    write_impact(&mut app, car, ObjectId::WORLD, 0.5);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.impacts, r.voices, r.failed), (0, 0, 0));
    assert!(impact_voices(&mut app).is_empty());
}

#[test]
fn the_struck_props_audio_id_selects_its_category() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    let (prop, _) = spawn_object(
        &mut app,
        Banger::new(BangerDefinition {
            name: "sp_testprop".into(),
            mass: 40.0,
            friction: 0.9,
            elasticity: 0.5,
            impulse_limit2: 0.0,
            size: [0.5, 0.5, 0.5],
            cg: [0.0, 0.0, 0.0],
            num_parts: 0,
            audio_id: 7,
        }),
    );
    write_impact(&mut app, car, prop, 3.0);
    app.update();
    // The prop's AudioId 7 lands the LIGHT category → PROP's 11 kHz
    // sample — and only the car's side voices (the prop is no vehicle).
    assert_eq!(impact_voices(&mut app), [(VoiceKind::Impact, 11025, false)]);
}

#[test]
fn a_two_vehicle_impact_voices_each_side_at_its_own_impulse() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (local, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    let (ai, _) = spawn_test_car(&mut app, PlayerControl::Ai, 3000.0);
    // 3 m/s: the 1300 kg car reads force 3900 → SOFT (22 kHz); the
    // 3000 kg car reads 9000 → HUGE (48 kHz) — per-car impulse, not a
    // shared pick.
    write_impact(&mut app, local, ai, 3.0);
    app.update();
    let voices = impact_voices(&mut app);
    assert_eq!(
        voices,
        [
            (VoiceKind::Impact, 22050, false),
            (VoiceKind::Impact, 48000, true)
        ],
        "the AI car's voice is a spatial emitter at the contact"
    );
    assert_eq!(app.world().resource::<AudioReport>().impacts, 2);
}

#[test]
fn a_remote_participant_spawns_no_voice_on_a_predicted_client() {
    // A Remote copy's local-stream hit stays silent on a client — the
    // same impact arrives as a `RemoteImpact` row off the snap tail, so
    // voicing here too would double the sound.
    let dir = impact_dir();
    let mut app = impact_app_with(dir.path(), SessionAuthority::Remote);
    let (remote, _) = spawn_test_car(&mut app, PlayerControl::Remote, 1300.0);
    write_impact(&mut app, remote, ObjectId::WORLD, 30.0);
    app.update();
    assert!(impact_voices(&mut app).is_empty());
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.impacts, r.failed), (0, 0));
}

#[test]
fn a_remote_participant_voices_on_the_authority() {
    // On the authority the remote seat is a locally simulated
    // participant — its impacts voice here like an AI car's, spatially
    // at the contact.
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (remote, _) = spawn_test_car(&mut app, PlayerControl::Remote, 1300.0);
    write_impact(&mut app, remote, ObjectId::WORLD, 30.0);
    app.update();
    // 30 m/s × 1300 kg → HUGE band, spatially at the contact point.
    assert_eq!(impact_voices(&mut app), [(VoiceKind::Impact, 48000, true)]);
    assert_eq!(app.world().resource::<AudioReport>().impacts, 1);
}

#[test]
fn a_replicated_impact_voices_the_remote_copy() {
    // The v10 `Snap.impacts` path: a resolved `RemoteImpact` voices the
    // copy's side — spatial at the contact, a wire `audio_id` of 0
    // reading the catch-all like a world/recordless struck side does
    // locally.
    let dir = impact_dir();
    let mut app = impact_app_with(dir.path(), SessionAuthority::Remote);
    let (_, entity) = spawn_test_car(&mut app, PlayerControl::Remote, 1300.0);
    app.world_mut()
        .write_message(mm2_app::netdrive::RemoteImpact {
            entity,
            point: Vec3::new(1.0, 2.0, 3.0),
            normal: Vec3::Y,
            severity: 30.0,
            audio_id: 0,
        });
    app.update();
    assert_eq!(impact_voices(&mut app), [(VoiceKind::Impact, 48000, true)]);
    assert_eq!(app.world().resource::<AudioReport>().impacts, 1);
}

#[test]
fn a_replicated_impact_picks_the_struck_sides_authored_category() {
    // Protocol v12: the authority resolved the struck prop's authored
    // `AudioId` into the row — the client voices the same LIGHT
    // category (`PROP`, 11 kHz) the authority played, not the id-0
    // catch-all a wire without the selector would have picked (30 m/s
    // × 1300 kg = 39000 force → HUGE under WALL, PROP under LIGHT).
    let dir = impact_dir();
    let mut app = impact_app_with(dir.path(), SessionAuthority::Remote);
    let (_, entity) = spawn_test_car(&mut app, PlayerControl::Remote, 1300.0);
    app.world_mut()
        .write_message(mm2_app::netdrive::RemoteImpact {
            entity,
            point: Vec3::new(1.0, 2.0, 3.0),
            normal: Vec3::Y,
            severity: 30.0,
            audio_id: 7,
        });
    app.update();
    assert_eq!(impact_voices(&mut app), [(VoiceKind::Impact, 11025, true)]);
    assert_eq!(app.world().resource::<AudioReport>().impacts, 1);
}

#[test]
fn events_without_a_vehicle_participant_stay_silent() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (prop_a, _) = spawn_object(
        &mut app,
        Banger::new(BangerDefinition {
            name: "sp_a".into(),
            mass: 40.0,
            friction: 0.9,
            elasticity: 0.5,
            impulse_limit2: 0.0,
            size: [0.5, 0.5, 0.5],
            cg: [0.0, 0.0, 0.0],
            num_parts: 0,
            audio_id: 7,
        }),
    );
    // A knocked prop meeting the world: no striker car, no authored
    // car-audio sample — the event is consumed without a voice.
    write_impact(&mut app, prop_a, ObjectId::WORLD, 30.0);
    app.update();
    assert!(impact_voices(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().impacts, 0);
}

#[test]
fn the_impact_voice_bound_caps_pile_ups() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    for i in 0..20u64 {
        let generation = app.world().resource::<Session>().generation();
        app.world_mut().write_message(ImpactEvent {
            id: ImpactId(i + 1),
            generation,
            tick: 0,
            participants: (car, ObjectId::WORLD),
            point: Vec3::ZERO,
            normal: Vec3::Y,
            severity: 3.0,
            surface: SurfaceState::default(),
        });
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.impacts, 12, "MAX_IMPACT_VOICES bounds the pile-up");
    assert_eq!(r.dropped, 8);
    assert_eq!(impact_voices(&mut app).len(), 12);
}

#[test]
fn a_stale_generation_event_is_skipped() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    let generation = app.world().resource::<Session>().generation();
    app.world_mut().write_message(ImpactEvent {
        id: ImpactId(1),
        generation: generation + 1,
        tick: 0,
        participants: (car, ObjectId::WORLD),
        point: Vec3::ZERO,
        normal: Vec3::Y,
        severity: 30.0,
        surface: SurfaceState::default(),
    });
    app.update();
    assert!(impact_voices(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().impacts, 0);
}

#[test]
fn teardown_sweeps_impact_voices_with_the_session() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    write_impact(&mut app, car, ObjectId::WORLD, 3.0);
    app.update();
    assert_eq!(impact_voices(&mut app).len(), 1);

    // The production teardown sweeps `SessionEntity` roots on
    // Unloading — the free-standing voice is one (AC06's restart leg).
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert!(impact_voices(&mut app).is_empty());
}

#[test]
fn an_absent_table_degrades_to_silence() {
    let dir = fixture_dir(); // waves only — no impact table
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    assert!(ImpactAudio::load(&vfs, 1).is_none());
    let mut app = impact_app(dir.path());
    assert!(app.world().get_resource::<ImpactAudio>().is_none());
    let (car, _) = spawn_test_car(&mut app, PlayerControl::Local, 1300.0);
    write_impact(&mut app, car, ObjectId::WORLD, 30.0);
    app.update();
    assert!(impact_voices(&mut app).is_empty());
}

// ---------------------------------------------------------------------------
// F07-B.4: grounded-wheel contact → bounded skid/rolling loop voices.
// The session `SurfaceAudio` holds the authored
// `default_surfacedry.csv`; a collider's `SurfaceMaterial` → the
// material's `sound` class picks the row, `tire_slippage`/`|vel_long|`
// picks the band and `|forward_speed|` mixes the rolling loop.
// ---------------------------------------------------------------------------

/// The authored dry table rows — the retail `default_surfacedry.csv`
/// 10-column schema. Row 0 is the `_default` road (NOSOUND rolling +
/// two slippage bands), row 1 `grass` (a rolling loop + one wide
/// band). Shared with the net legs as `tests/support::DRY_SURFACE_TABLE`.
use support::DRY_SURFACE_TABLE as DRY_TABLE;

/// The wet table — same schema as dry (AUD-6), distinct sample names
/// so a resolved voice identifies which variant bound.
const WET_TABLE: &[u8] = b"Tunnel sound index\n0\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
NOSOUND,125,0,0,0,0,0.5,0.88,2\n\
skid wave,min slippage,max slippage\n\
WETSKID,0.5,0.75\n\
WETSKID,0.75,1\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
WETROLL,25,0.35,0.75,0.85,1.25,0.5,0.72,1\n\
skid wave,min slippage,max slippage\n\
WETSKID,0.25,1\n";

/// The surface fixture: both player-side surface variants
/// (`default_surfacedry.csv`/`default_surfacewet.csv`) plus every wave
/// they name — at distinguishing sample rates so a voice's resolved
/// asset identifies its authored row and table.
fn surface_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for (stem, rate) in [
        ("roadskid1", 22050),
        ("roadskid2", 32000),
        ("grassskid", 11025),
        ("rollwave", 48000),
        ("wetskid", 8000),
        ("wetroll", 16000),
    ] {
        write(
            d,
            &format!("aud/aud22/surfaces/{stem}.22k.wav"),
            &pcm_wav(rate, 220),
        );
    }
    write(d, "aud/cardata/player/default_surfacedry.csv", DRY_TABLE);
    write(d, "aud/cardata/player/default_surfacewet.csv", WET_TABLE);
    tmp
}

/// `_default` → row 0, `grass` → row 1 — the `sound` class wiring
/// `SurfaceTables::sound_index` reads.
fn surface_tables() -> SurfaceTables {
    let set = MaterialSet::parse("mtl _default {\n    sound: 0\n}\nmtl grass {\n    sound: 1\n}\n")
        .unwrap();
    let map = MaterialMap::parse("texture,physics\n").unwrap();
    SurfaceTables { set, map }
}

/// The audio slice of the production app for surface voices: a
/// `Playing` session, the mounted fixture VFS, the session `WaveBank`,
/// `SurfaceAudio` (loaded through the production path), the material
/// tables the collider marks read, and `surface_voices` on Update like
/// the live schedules wire it.
fn surface_app(dir: &Path) -> App {
    surface_app_for(dir, Weather::default(), None)
}

/// `surface_app` with the session's weather selector and player
/// vehicle id the production `SurfaceAudio::load` reads (F07-B.8).
fn surface_app_for(dir: &Path, weather: Weather, vehicle: Option<&str>) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    // Production inserts the resource only when the authored table
    // loads — an absent record leaves none and degrades to silence.
    let table = SurfaceAudio::load(&vfs, weather, vehicle);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .insert_resource(surface_tables())
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(Update, audio::surface_voices);
    if let Some(table) = table {
        app.insert_resource(table);
    }
    app.finish();
    app.cleanup();
    app
}

/// A drivable car like `load_session_world`/`spawn_opponents` stamps
/// (`local` carries `PlayerVehicle`), plus the collider entity whose
/// `SurfaceMaterial` the wheel contacts report.
fn surface_car(app: &mut App, local: bool, material: SurfaceMaterial) -> (Entity, Entity) {
    let generation = app.world().resource::<Session>().generation();
    let collider = app.world_mut().spawn(material).id();
    let mut car = app.world_mut().spawn((
        vehicle_bundle(&VehicleConfig::default()),
        SessionEntity(generation),
        Transform::default(),
    ));
    if local {
        car.insert(PlayerVehicle);
    }
    (car.id(), collider)
}

/// Point every wheel at `collider` with the given telemetry — the
/// fields `surface_voices` reads, written on the sim state like the
/// tire model does.
fn set_contact(app: &mut App, car: Entity, contact: Option<Entity>, speed: f32, slip: f32) {
    let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
    state.forward_speed = speed;
    for w in &mut state.wheels {
        w.grounded = contact.is_some();
        w.contact_entity = contact;
        w.slip_angle = slip;
        w.vel_long = speed;
    }
}

/// `(role, resolved sample rate, spatial, parent)` for every live
/// surface voice — the rate identifies the authored row through the
/// fixture's distinguishing waves.
fn surface_voices(app: &mut App) -> Vec<(SurfaceRole, u32, bool, Entity)> {
    let world = app.world_mut();
    let mut q = world.query::<(
        &SurfaceVoice,
        &AudioPlayer<PcmAudio>,
        &PlaybackSettings,
        &ChildOf,
    )>();
    let mut out: Vec<_> = q
        .iter(world)
        .map(|(v, p, s, c)| {
            let rate = world
                .resource::<Assets<PcmAudio>>()
                .get(&p.0)
                .unwrap()
                .sample_rate
                .get();
            (v.role, rate, s.spatial, c.parent())
        })
        .collect();
    out.sort_by_key(|(role, rate, ..)| {
        (
            *rate,
            match role {
                SurfaceRole::Skid(b) => *b as u32,
                SurfaceRole::Rolling => u32::MAX,
            },
        )
    });
    out
}

fn surface_mix(app: &mut App, role: SurfaceRole) -> (f32, f32) {
    app.world_mut()
        .query::<&SurfaceVoice>()
        .iter(app.world())
        .find(|v| v.role == role)
        .map(|v| (v.mix.volume, v.mix.speed))
        .unwrap_or((0.0, 0.0))
}

#[test]
fn a_sliding_wheel_voices_the_covering_skid_band() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, road) = surface_car(&mut app, true, SurfaceMaterial::Unspecified);
    // A lateral slide at 0.6 utilization lands the `_default` row's
    // first authored band (0.5–0.75).
    set_contact(&mut app, car, Some(road), 8.0, 0.6 * 0.16);
    app.update(); // resolve → rig
    app.update(); // commit the entry + spawn the band voice
    app.update(); // the voice's first computed mix

    let voices = surface_voices(&mut app);
    assert_eq!(
        voices,
        [(SurfaceRole::Skid(0), 22050, false, car)],
        "the covering band's own wave — non-spatial for the local car"
    );
    // progress 0.4 across the band interpolates min→max skid volume.
    let (volume, speed) = surface_mix(&mut app, SurfaceRole::Skid(0));
    assert!((volume - 0.652).abs() < 1e-3, "{volume}");
    assert_eq!(speed, 1.0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.skids, r.rolling, r.voices, r.failed), (1, 0, 1, 0));

    // The voice rides the car (despawns with it) and carries the
    // session stamp teardown owns.
    let world = app.world_mut();
    let (mode, session_entity) = world
        .query::<(&PlaybackSettings, &SessionEntity)>()
        .iter(world)
        .next()
        .map(|(s, e)| (s.mode, e.0))
        .unwrap();
    assert!(matches!(mode, PlaybackMode::Loop));
    assert_eq!(
        session_entity,
        app.world().resource::<Session>().generation()
    );
}

/// F25-B (protocol v16): the `SurfaceContact` `contact_pick` writes on
/// a live car is the wire's source — assert the resolved record itself
/// (class, slippage, wheel speed, rolling class), not just the voices
/// it produced, and that a quiet resolve writes the empty contact back
/// so the publish encodes its sentinels rather than a stale row.
#[test]
fn a_live_car_records_the_resolved_contact_for_the_wire() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    // Rolling at 10 m/s plus a slide 0.6 of the way past the
    // longitudinal limit — both halves of the grass row resolve.
    set_contact(&mut app, car, Some(grass), 10.0, 0.0);
    let ratio = app
        .world()
        .get::<Vehicle>(car)
        .unwrap()
        .config
        .tires
        .peak_slip_ratio;
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
        for w in &mut state.wheels {
            w.traction_demand = 1.0 + 0.6 * ratio;
        }
    }
    app.update();

    let contact = app
        .world()
        .get::<SurfaceContact>(car)
        .expect("the resolve wrote the car's contact");
    let skid = contact.skid.expect("the slide resolved a skid contact");
    assert_eq!(skid.surface, 1, "the collider's authored sound class");
    assert!(
        (skid.slippage - 0.6).abs() < 1e-5,
        "the winning wheel's slide: {}",
        skid.slippage
    );
    assert_eq!(skid.wheel_speed, 10.0, "the winning wheel's vel_long");
    assert_eq!(contact.roll, Some(1), "the moving car's rolling class");

    // Airborne: the next resolve writes the empty contact — the record
    // a quiet frame publishes.
    set_contact(&mut app, car, None, 10.0, 0.9);
    app.update();
    let contact = app
        .world()
        .get::<SurfaceContact>(car)
        .expect("the component stays bound");
    assert!(
        contact.skid.is_none() && contact.roll.is_none(),
        "an unresolving frame clears the record, not a stale carry-over"
    );
}

#[test]
fn a_held_brake_at_rest_and_airborne_wheels_stay_silent() {
    // F07-AC03's two legs. A parked car fully on the brakes: the wheels
    // are grounded and contacting a real surface, but demand nothing —
    // no band covers and the roll gate is closed.
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    set_contact(&mut app, car, Some(grass), 0.0, 0.0);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(voices(&mut app), 0);
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<SurfaceRig>>()
            .iter(app.world())
            .count(),
        0,
        "nothing resolved → no rig was ever built"
    );

    // Airborne at full lock and hard demand: no contact means no
    // surface row — the telemetry cannot reach a pick.
    set_contact(&mut app, car, None, 20.0, 0.9);
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
        for w in &mut state.wheels {
            w.traction_demand = 1.2;
        }
    }
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(voices(&mut app), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.voices, r.failed, r.skids, r.rolling), (0, 0, 0, 0));
}

#[test]
fn a_moving_car_rolls_the_authored_surface_loop() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    let (ai, _) = surface_car(&mut app, false, SurfaceMaterial::Authored(1));
    for c in [car, ai] {
        set_contact(&mut app, c, Some(grass), 10.0, 0.0);
    }
    for _ in 0..3 {
        app.update();
    }

    let mut voices = surface_voices(&mut app);
    voices.sort_by_key(|(.., parent)| *parent != car);
    assert_eq!(
        voices,
        [
            (SurfaceRole::Rolling, 48000, false, car),
            (SurfaceRole::Rolling, 48000, true, ai)
        ],
        "the AI car's loop is a spatial emitter, the local car's is not"
    );
    // 10 m/s across the authored 0–25 window: f=0.4 → volume 0.35→0.75,
    // pitch 0.85→1.25.
    let (volume, speed) = surface_mix(&mut app, SurfaceRole::Rolling);
    assert!((volume - 0.51).abs() < 1e-3, "{volume}");
    assert!((speed - 1.01).abs() < 1e-3, "{speed}");
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.rolling, r.skids, r.failed), (2, 0, 0));
}

#[test]
fn a_surface_switch_rebuilds_the_skid_after_the_dwell() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, road) = surface_car(&mut app, true, SurfaceMaterial::Unspecified);
    let grass = app.world_mut().spawn(SurfaceMaterial::Authored(1)).id();

    set_contact(&mut app, car, Some(road), 8.0, 0.6 * 0.16);
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        surface_voices(&mut app),
        [(SurfaceRole::Skid(0), 22050, false, car)]
    );

    // Sliding onto grass keeps the road voice (idled at 0) until the
    // new entry has held the dwell, then the grass band replaces it.
    set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    for _ in 0..3 {
        app.update();
    }
    let voices = surface_voices(&mut app);
    assert!(
        voices
            .iter()
            .any(|v| matches!(v.0, SurfaceRole::Skid(_)) && v.1 == 22050),
        "inside the dwell the committed road band still owns the slot: {voices:?}"
    );

    for _ in 0..3 {
        app.update();
    }
    let voices = surface_voices(&mut app);
    assert_eq!(
        voices,
        [
            (SurfaceRole::Skid(0), 11025, false, car),
            (SurfaceRole::Rolling, 48000, false, car)
        ],
        "the grass band's own wave replaced the road skid; the rolling \
         loop joined once the car was moving on grass"
    );
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.skids, r.rolling, r.failed), (1, 1, 0));
}

/// F06-AC05: a collider whose material index the table cannot answer
/// (a stale or wire-borne value — import never produces one) is the
/// `_default` surface to the audio path, exactly as it is to the tire
/// and particle paths, rather than a wheel that sounds nothing while
/// its tire reads neutral.
#[test]
fn a_dead_material_index_voices_as_the_default_surface() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, collider) = surface_car(&mut app, true, SurfaceMaterial::Authored(9));
    set_contact(&mut app, car, Some(collider), 8.0, 0.6 * 0.16);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        surface_voices(&mut app),
        [(SurfaceRole::Skid(0), 22050, false, car)],
        "the `_default` row's covering band, as an unmarked collider sounds"
    );
    assert_eq!(app.world().resource::<AudioReport>().failed, 0);
}

#[test]
fn an_absent_table_stays_silent() {
    // No authored table at all — `load` refuses and the system idles.
    let dir = fixture_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    assert!(SurfaceAudio::load(&vfs, Weather::default(), None).is_none());
    let mut app = surface_app(dir.path());
    assert!(app.world().get_resource::<SurfaceAudio>().is_none());
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(voices(&mut app), 0);
    assert_eq!(app.world().resource::<AudioReport>().voices, 0);
}

#[test]
fn the_surface_rig_bound_caps_resolving_cars() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let grass = app.world_mut().spawn(SurfaceMaterial::Authored(1)).id();
    // Twenty cars all rolling-and-sliding on grass: sixteen rigs build,
    // four report the bound once and stay muted.
    for i in 0..20 {
        let (car, _) = surface_car(&mut app, i == 0, SurfaceMaterial::Authored(1));
        set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    }
    for _ in 0..3 {
        app.update();
    }
    {
        let r = app.world().resource::<AudioReport>();
        assert_eq!(r.dropped, 4, "MAX_SURFACE_RIGS refused four cars");
        assert_eq!(r.failed, 0);
        assert_eq!((r.skids, r.rolling), (16, 16));
    }
    // Sixteen rigs × (one skid band + one rolling loop).
    assert_eq!(surface_voices(&mut app).len(), 32);

    app.update();
    assert_eq!(
        app.world().resource::<AudioReport>().dropped,
        4,
        "the muted marker reports once, not per frame"
    );
}

/// F25-B (protocol v16): a `RemoteReplica` copy keeps no wheel-contact
/// truth — a kinematic replica's wheels never ground — so
/// `surface_voices` replays the replicated `SurfaceContact` through
/// *this* process's `SurfaceAudio` instead of reading wheels
/// (`aud/` rides no gameplay fingerprint; each peer re-runs the pick).
/// A class the local table cannot answer resolves to silence like an
/// unmapped material — no fabricated row.
#[test]
fn a_remote_replica_replays_its_replicated_surface_contact() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    // The copy's only contact truth is the replicated component —
    // `set_contact(None, ..)` leaves every wheel airborne, so the
    // live-wheel loop could never produce what voices here.
    let (copy, _collider) = surface_car(&mut app, false, SurfaceMaterial::Unspecified);
    app.world_mut().entity_mut(copy).insert((
        RemoteReplica,
        SurfaceContact {
            skid: Some(SkidContact {
                surface: 99,
                slippage: 0.9,
                wheel_speed: -8.0,
            }),
            roll: Some(99),
        },
    ));
    // `apply_present`'s wire-derived forward speed — 10 m/s keeps the
    // rolling gate open.
    set_contact(&mut app, copy, None, 10.0, 0.0);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        surface_voices(&mut app),
        [],
        "a surface class the local table cannot answer stays silent"
    );
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<SurfaceRig>>()
            .iter(app.world())
            .count(),
        0,
        "no rig builds off an unresolvable contact"
    );

    // The v16 tail lands a real contact: `_default` row 0's skid band
    // 0 covers 0.5–0.75 slippage (ROADSKID1's 22050 wave) and class 1's
    // grass rolling loop (ROLLWAVE's 48000 wave) mixes off the copy's
    // own forward speed. Non-player → both spatial emitters.
    app.world_mut().entity_mut(copy).insert(SurfaceContact {
        skid: Some(SkidContact {
            surface: 0,
            slippage: 0.6,
            wheel_speed: -8.0,
        }),
        roll: Some(1),
    });
    for _ in 0..3 {
        app.update();
    }
    let mut voices = surface_voices(&mut app);
    voices.sort_by_key(|(role, ..)| match role {
        SurfaceRole::Skid(b) => *b,
        SurfaceRole::Rolling => usize::MAX,
    });
    assert_eq!(
        voices,
        [
            (SurfaceRole::Skid(0), 22050, true, copy),
            (SurfaceRole::Rolling, 48000, true, copy)
        ],
        "the replicated contact replays through the local table — \
         spatial like any non-player car"
    );
    // Band 0 progress 0.4 (0.6 across 0.5–0.75) interpolates the
    // authored skid volumes like the live leg's numbers.
    let (volume, speed) = surface_mix(&mut app, SurfaceRole::Skid(0));
    assert!((volume - 0.652).abs() < 1e-3, "{volume}");
    assert_eq!(speed, 1.0);
    // Rolling at 10 m/s across the authored 0–25 window — the copy's
    // `apply_present` speed, not a wire field.
    let (volume, speed) = surface_mix(&mut app, SurfaceRole::Rolling);
    assert!((volume - 0.51).abs() < 1e-3, "{volume}");
    assert!((speed - 1.01).abs() < 1e-3, "{speed}");
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.skids, r.rolling, r.failed), (1, 1, 0));

    // A quiet snap (both tails `SNAP_NO_SURFACE` → `None`) silences
    // the copy like an authority-side release.
    app.world_mut()
        .entity_mut(copy)
        .insert(SurfaceContact::default());
    for _ in 0..3 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.skids, r.rolling), (0, 0));
}

#[test]
fn teardown_sweeps_surface_voices_with_the_session() {
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    for _ in 0..3 {
        app.update();
    }
    assert!(!surface_voices(&mut app).is_empty());

    // The production teardown sweeps `SessionEntity` roots on Unloading
    // — every surface voice is one (AC06's restart leg).
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

// ---------------------------------------------------------------------------
// F07-B.8: the session's effective weather binds the surface-table
// variant — `rainy` → `default_surfacewet.csv`, every other authored
// selector → `default_surfacedry.csv` (designed, DSN-43; the exe
// carries only the dry/wet strings — `surfaceice` is dead data,
// AUD-11). A `<vehicle>_surface<variant>` file probes ahead of the
// shared default (the exe's `%s_` format, stem binding inferred).
// ---------------------------------------------------------------------------

#[test]
fn rainy_weather_binds_the_wet_surface_table() {
    let dir = surface_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let rainy = Weather::new(3).unwrap();
    let audio = SurfaceAudio::load(&vfs, rainy, None).unwrap();
    assert_eq!(audio.variant, SurfaceVariant::Wet);
    assert_eq!(audio.path, "aud/cardata/player/default_surfacewet.csv");

    // End to end: a rolling grass contact voices the wet table's
    // authored samples — the wet rolling loop (rate 16000) and wet
    // skid band (rate 8000), not the dry row's (48000/11025).
    let mut app = surface_app_for(dir.path(), rainy, None);
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    for _ in 0..3 {
        app.update();
    }
    let vs = surface_voices(&mut app);
    assert!(
        vs.iter()
            .any(|(role, rate, ..)| *role == SurfaceRole::Rolling && *rate == 16000),
        "wet rolling loop: {vs:?}"
    );
    assert!(
        vs.iter()
            .any(|(role, rate, ..)| matches!(role, SurfaceRole::Skid(_)) && *rate == 8000),
        "wet skid band: {vs:?}"
    );
}

#[test]
fn dry_weathers_bind_the_dry_surface_table() {
    let dir = surface_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    for w in 0..=2u8 {
        let audio = SurfaceAudio::load(&vfs, Weather::new(w).unwrap(), None).unwrap();
        assert_eq!(audio.variant, SurfaceVariant::Dry, "selector {w}");
        assert_eq!(audio.path, "aud/cardata/player/default_surfacedry.csv");
    }
}

#[test]
fn a_per_vehicle_surface_table_wins_over_the_default() {
    let dir = surface_dir();
    // A mod-style per-vehicle override authors its own rolling wave —
    // the `%s_surface<variant>` probe the exe's strings imply.
    write(
        dir.path(),
        "aud/aud22/surfaces/carroll.22k.wav",
        &pcm_wav(30000, 220),
    );
    write(
        dir.path(),
        "aud/cardata/player/testcar_surfacewet.csv",
        b"Tunnel sound index\n0\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
NOSOUND,125,0,0,0,0,0.5,0.88,1\n\
skid wave,min slippage,max slippage\n\
WETSKID,0.5,1\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
CARROLL,25,0.35,0.75,0.85,1.25,0.5,0.72,1\n\
skid wave,min slippage,max slippage\n\
WETSKID,0.25,1\n",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let rainy = Weather::new(3).unwrap();
    let audio = SurfaceAudio::load(&vfs, rainy, Some("testcar")).unwrap();
    assert_eq!(audio.variant, SurfaceVariant::Wet);
    assert_eq!(audio.path, "aud/cardata/player/testcar_surfacewet.csv");

    // And its authored sample is the one that resolves end to end.
    let mut app = surface_app_for(dir.path(), rainy, Some("testcar"));
    let (car, grass) = surface_car(&mut app, true, SurfaceMaterial::Authored(1));
    set_contact(&mut app, car, Some(grass), 8.0, 0.6 * 0.16);
    for _ in 0..3 {
        app.update();
    }
    let vs = surface_voices(&mut app);
    assert!(
        vs.iter()
            .any(|(role, rate, ..)| *role == SurfaceRole::Rolling && *rate == 30000),
        "per-vehicle rolling loop: {vs:?}"
    );
}

#[test]
fn a_malformed_vehicle_table_falls_through_to_the_default() {
    let dir = surface_dir();
    write(
        dir.path(),
        "aud/cardata/player/testcar_surfacewet.csv",
        b"this is not a cardata file",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let audio = SurfaceAudio::load(&vfs, Weather::new(3).unwrap(), Some("testcar")).unwrap();
    assert_eq!(audio.variant, SurfaceVariant::Wet);
    assert_eq!(audio.path, "aud/cardata/player/default_surfacewet.csv");
}

#[test]
fn a_missing_wet_table_is_no_substitute_for_dry() {
    // Only the dry table ships: a rainy session gets no surface audio
    // rather than the dry table standing in for the wet — the same
    // absence policy every authored record applies.
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "aud/cardata/player/default_surfacedry.csv",
        DRY_TABLE,
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    assert!(SurfaceAudio::load(&vfs, Weather::new(3).unwrap(), None).is_none());
    // The same tree under a dry selector binds normally.
    assert!(SurfaceAudio::load(&vfs, Weather::new(0).unwrap(), None).is_some());
}

// ---------------------------------------------------------------------------
// F07-B.5: committed gear/direction changes → authored clutch one-shot.
// `GearWatch` dedups the trigger: first sight is not a shift, one voice
// per committed `(gear, direction)` change, remote/sentinel legs stay
// silent.
// ---------------------------------------------------------------------------

/// A fixture install with the one wave the clutch binding names.
fn clutch_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "aud/aud22/clutch/gearclunk.22k.wav",
        &pcm_wav(22050, 220),
    );
    tmp
}

/// A cardata horn row authoring clutch `stem` at `vol` — no engine
/// rows, so the parsed table carries only the horn + clutch bindings.
fn clutch_car_audio(stem: &str, vol: f32) -> CarAudio {
    let csv = format!(
        "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nTESTHORN,0.9,0,0,{stem},{vol}\n"
    );
    CarAudio::parse(csv.as_bytes()).unwrap()
}

/// The audio slice of the production app for clutch voices: a
/// `Playing` session, the mounted fixture VFS, the session `WaveBank`
/// and `clutch_voices` on Update like the live schedules wire it.
fn clutch_app(dir: &Path) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(Update, audio::clutch_voices);
    app.finish();
    app.cleanup();
    app
}

/// A drivable car like `spawn_test_car` stamps, plus the cardata
/// binding and session ownership `load_session_world` adds.
fn clutch_car(app: &mut App, control: PlayerControl, spec: CarAudio) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let (_, car) = spawn_test_car(app, control, 1300.0);
    app.world_mut()
        .entity_mut(car)
        .insert((VehicleAudio { spec }, SessionEntity(generation)));
    car
}

/// `(spatial, parent)` for every live clutch voice.
fn clutch_voice_list(app: &mut App) -> Vec<(bool, Entity)> {
    let world = app.world_mut();
    let mut q = world.query::<(&AudioVoice, &PlaybackSettings, &ChildOf)>();
    let mut out: Vec<_> = q
        .iter(world)
        .filter(|(v, ..)| v.kind == VoiceKind::Clutch)
        .map(|(_, s, c)| (s.spatial, c.parent()))
        .collect();
    out.sort_by_key(|(_, p)| p.index());
    out
}

#[test]
fn a_gear_change_voices_the_authored_clutch() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("gearclunk", 0.7),
    );
    let ai = clutch_car(
        &mut app,
        PlayerControl::Ai,
        clutch_car_audio("gearclunk", 0.7),
    );

    // First sight is not a shift — the watch lands silently.
    app.update();
    assert!(app.world().get::<GearWatch>(car).is_some());
    assert_eq!(voices(&mut app), 0);

    app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    app.world_mut().get_mut::<VehicleState>(ai).unwrap().gear = 1;
    app.update();

    // Two voices, one per car; the local player's anchors the mix
    // non-spatially, the AI car's is a spatial emitter (DSN-37).
    let mut list = clutch_voice_list(&mut app);
    list.sort_by_key(|(_, p)| *p != car);
    assert_eq!(list, [(false, car), (true, ai)]);
    let world = app.world_mut();
    let (kind, player, settings, session_entity) = world
        .query::<(
            &AudioVoice,
            &AudioPlayer<PcmAudio>,
            &PlaybackSettings,
            &SessionEntity,
        )>()
        .iter(world)
        .next()
        .map(|(v, p, s, e)| (v.kind, p.0.clone(), *s, e.0))
        .unwrap();
    assert_eq!(kind, VoiceKind::Clutch);
    assert!(matches!(settings.mode, PlaybackMode::Despawn));
    assert_eq!(settings.volume, Volume::Linear(0.7));
    assert_eq!(
        session_entity,
        app.world().resource::<Session>().generation()
    );
    let waves = app.world().resource::<Assets<PcmAudio>>();
    assert_eq!(waves.get(&player).unwrap().sample_rate.get(), 22050);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.clutch, r.voices, r.failed), (2, 2, 0));
}

#[test]
fn a_multi_gear_jump_and_an_unchanged_gear_do_not_retrigger() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("gearclunk", 0.7),
    );
    app.update();

    // The selector committing 0 → 3 in one step is a single clutch
    // actuation — one voice, not three.
    app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 3;
    app.update();
    assert_eq!(clutch_voice_list(&mut app).len(), 1);

    // Holding the gear produces nothing further.
    app.update();
    assert_eq!(clutch_voice_list(&mut app).len(), 1);
    assert_eq!(app.world().resource::<AudioReport>().clutch, 1);
}

#[test]
fn a_reverse_engagement_voices_the_clutch() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("gearclunk", 0.7),
    );
    app.update();

    app.world_mut()
        .get_mut::<VehicleState>(car)
        .unwrap()
        .direction = DriveDirection::Reverse;
    app.update();
    assert_eq!(clutch_voice_list(&mut app).len(), 1);

    // Back out of reverse is a second committed change.
    app.world_mut()
        .get_mut::<VehicleState>(car)
        .unwrap()
        .direction = DriveDirection::Forward;
    app.update();
    assert_eq!(clutch_voice_list(&mut app).len(), 2);
    assert_eq!(app.world().resource::<AudioReport>().clutch, 2);
}

#[test]
fn a_remote_car_shifts_silently() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Remote,
        clutch_car_audio("gearclunk", 0.7),
    );
    app.update();

    app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    app.update();
    // The remote car's audio belongs to its own client — the watch
    // still advanced, so no voice flushes later either.
    assert_eq!(voices(&mut app), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.clutch, r.failed), (0, 0));
}

#[test]
fn a_sentinel_clutch_is_silence_and_a_missing_wave_reports() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let quiet = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("NOSOUND", 0.7),
    );
    let broken = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("nosuchstem", 0.7),
    );
    app.update();

    for car in [quiet, broken] {
        app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    }
    app.update();
    // Authored silence is not a failure; an unresolvable stem counts
    // once per change.
    assert_eq!(voices(&mut app), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.clutch, r.voices, r.failed), (0, 0, 1));
}

#[test]
fn the_clutch_voice_bound_caps_a_field_shift() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let mut cars = Vec::new();
    for _ in 0..10 {
        cars.push(clutch_car(
            &mut app,
            PlayerControl::Ai,
            clutch_car_audio("gearclunk", 0.7),
        ));
    }
    app.update(); // watches land
    for car in cars {
        app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.clutch, 8, "MAX_CLUTCH_VOICES bounds the burst");
    assert_eq!(r.dropped, 2);
    assert_eq!(clutch_voice_list(&mut app).len(), 8);
}

#[test]
fn a_paused_shift_is_observed_not_voiced() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("gearclunk", 0.7),
    );
    app.update();

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    app.update();
    assert_eq!(voices(&mut app), 0, "a paused session voices nothing");

    // Resuming does not flush the buffered change as a stale clunk —
    // the watch already committed it.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
    assert_eq!(app.world().resource::<AudioReport>().clutch, 0);
}

#[test]
fn teardown_sweeps_clutch_voices_with_the_session() {
    let dir = clutch_dir();
    let mut app = clutch_app(dir.path());
    let car = clutch_car(
        &mut app,
        PlayerControl::Local,
        clutch_car_audio("gearclunk", 0.7),
    );
    app.update();
    app.world_mut().get_mut::<VehicleState>(car).unwrap().gear = 1;
    app.update();
    assert_eq!(clutch_voice_list(&mut app).len(), 1);

    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

// ---------------------------------------------------------------------------
// F07-B.6: ambient-traffic engine loops — the resolved `*_engine.csv`
// table → one bounded spatial loop per `AmbientAudio` car, pitched
// through the authored speed bands off `LinearVelocity`.
// ---------------------------------------------------------------------------

/// A `default_engine.csv`-shaped table: two tight bands ahead of the
/// authored `0–500` catch-all — the overlap retail relies on.
fn ambient_table(sample: &str) -> String {
    format!(
        "Engine sample,engine volume,,\r\n{sample},0.9,,\r\nmin speed,max speed,engine min pitch,engine max pitch\r\n0,15,0.5,1.0\r\n15,40,1.0,1.4\r\n0,500,0.5,4.0\r\n"
    )
}

/// The resolved spec for a table naming `sample`.
fn ambient_spec(sample: &str) -> AmbientEngineSpec {
    let table = AmbientEngine::parse(ambient_table(sample).as_bytes()).unwrap();
    AmbientEngineSpec::from_table(&table).unwrap()
}

/// The ambient stem's wave plus per-class and default tables on the
/// fixture tree — `va_sedan` authors its own, every other class falls
/// to the default.
fn ambient_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "aud/aud22/ambient/testamb.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud22/ambient/defnote.22k.wav", &pcm_wav(22050, 220));
    write(
        d,
        "aud/cardata/ambient/va_sedan_engine.csv",
        ambient_table("TESTAMB").as_bytes(),
    );
    write(
        d,
        "aud/cardata/ambient/default_engine.csv",
        ambient_table("DEFNOTE").as_bytes(),
    );
    tmp
}

/// The audio slice ambient cars see: session `Playing`, the fixture
/// VFS and bank, and the chained rig/drive pair `main` schedules.
fn ambient_app(dir: &Path) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(
            Update,
            (audio::ambient_engine_rigs, audio::ambient_engine_drive).chain(),
        );
    app.finish();
    app.cleanup();
    app
}

/// One ambient car as `spawn_ambient_car` leaves it: the resolved
/// table, a session stamp and `LinearVelocity` — no render model or
/// collider is needed to exercise the audio systems.
fn ambient_car(app: &mut App, spec: AmbientEngineSpec) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .spawn((
            AmbientAudio { spec },
            LinearVelocity::ZERO,
            SessionEntity(generation),
            Transform::default(),
        ))
        .id()
}

fn ambient_voice_list(app: &mut App) -> Vec<Entity> {
    let mut v: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<AmbientEngineVoice>>()
        .iter(app.world())
        .collect();
    v.sort();
    v
}

#[test]
fn ambient_table_resolution_prefers_the_class_file_then_the_default() {
    let dir = ambient_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();

    let own = mm2_content::ambient_engine_audio(&vfs, "va_sedan")
        .unwrap()
        .unwrap();
    assert_eq!(own.sample.name, "TESTAMB");
    // A class with no authored file reads the shared default table.
    let shared = mm2_content::ambient_engine_audio(&vfs, "va_taxi")
        .unwrap()
        .unwrap();
    assert_eq!(shared.sample.name, "DEFNOTE");
    // Neither file resolving is authored silence, not an error.
    std::fs::remove_file(dir.path().join("aud/cardata/ambient/default_engine.csv")).unwrap();
    assert!(
        mm2_content::ambient_engine_audio(&vfs, "va_taxi")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_malformed_ambient_table_errors_instead_of_falling_back() {
    let dir = ambient_dir();
    // A broken per-class file is a content defect — it must not
    // silently substitute the default and voice the wrong note.
    write(
        dir.path(),
        "aud/cardata/ambient/va_sedan_engine.csv",
        b"not a table at all",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    assert!(mm2_content::ambient_engine_audio(&vfs, "va_sedan").is_err());
}

#[test]
fn a_sentinel_ambient_sample_is_authored_silence() {
    let table = AmbientEngine::parse(ambient_table("NOSOUND").as_bytes()).unwrap();
    assert!(AmbientEngineSpec::from_table(&table).is_none());
}

#[test]
fn an_ambient_car_spawns_one_spatial_looping_voice() {
    let dir = ambient_dir();
    let mut app = ambient_app(dir.path());
    let car = ambient_car(&mut app, ambient_spec("testamb"));
    app.update();

    let generation = app.world().resource::<Session>().generation();
    let (kind, mix, mode, spatial, parent, entity_gen) = {
        let world = app.world_mut();
        let mut q = world.query::<(
            &AudioVoice,
            &AmbientEngineVoice,
            &AudioPlayer<PcmAudio>,
            &PlaybackSettings,
            &ChildOf,
            &SessionEntity,
        )>();
        let all: Vec<_> = q.iter(world).collect();
        assert_eq!(all.len(), 1);
        let (voice, amb_voice, _, settings, child_of, session_entity) = all[0];
        (
            voice.kind,
            amb_voice.mix,
            settings.mode,
            settings.spatial,
            child_of.parent(),
            session_entity.0,
        )
    };
    assert_eq!(kind, VoiceKind::AmbientEngine);
    assert!(matches!(mode, PlaybackMode::Loop));
    assert!(spatial, "an ambient car is a world emitter");
    assert_eq!(parent, car);
    assert_eq!(entity_gen, generation);
    // The resolved wave is the fixture's own clip; the authored
    // constant volume drives the loop at rest.
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.ambient, r.voices, r.failed), (1, 1, 0));
    assert_eq!(r.ambient_live, 1);
    assert_eq!(mix.volume, 0.9);
    assert_eq!(mix.speed, 0.5, "band 0's low-edge pitch");
    assert!(app.world().get::<AmbientRig>(car).is_some());
}

#[test]
fn the_ambient_mix_follows_the_authored_speed_bands() {
    let dir = ambient_dir();
    let mut app = ambient_app(dir.path());
    let car = ambient_car(&mut app, ambient_spec("testamb"));
    app.update();

    let mix_speed = |app: &mut App, v: Vec3| -> f32 {
        *app.world_mut().get_mut::<LinearVelocity>(car).unwrap() = LinearVelocity(v);
        app.update();
        app.world_mut()
            .query::<&AmbientEngineVoice>()
            .single(app.world())
            .unwrap()
            .mix
            .speed
    };
    // Inside band 0 the catch-all is shadowed — authored order wins.
    assert_eq!(
        mix_speed(&mut app, Vec3::new(0.0, 0.0, 10.0)),
        0.5 + 0.5 * (10.0 / 15.0)
    );
    // Band 1 owns the mid range; a reverse velocity reads as speed.
    let forward = mix_speed(&mut app, Vec3::new(0.0, 0.0, -20.0));
    assert_eq!(forward, 1.0 + 0.4 * (5.0 / 25.0));
    // Above the tight bands only the catch-all covers.
    assert_eq!(
        mix_speed(&mut app, Vec3::new(0.0, 0.0, 100.0)),
        0.5 + 3.5 * (100.0 / 500.0)
    );
    // Volume never changes — the table authors no speed→volume term.
    let v = app
        .world_mut()
        .query::<&AmbientEngineVoice>()
        .single(app.world())
        .unwrap()
        .mix
        .volume;
    assert_eq!(v, 0.9);
}

#[test]
fn an_unresolvable_ambient_sample_fails_once_per_car() {
    let dir = ambient_dir();
    let mut app = ambient_app(dir.path());
    let car = ambient_car(&mut app, ambient_spec("nosuchstem"));
    app.update();

    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.ambient, r.voices, r.failed), (0, 0, 1));
    assert!(ambient_voice_list(&mut app).is_empty());
    // The rig marker suppresses a per-frame retry and re-warn.
    assert!(app.world().get::<AmbientRig>(car).is_some());
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().failed, 1);
}

#[test]
fn the_ambient_voice_bound_caps_the_fleet() {
    let dir = ambient_dir();
    let mut app = ambient_app(dir.path());
    for _ in 0..34 {
        ambient_car(&mut app, ambient_spec("testamb"));
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.ambient, 32, "MAX_AMBIENT_VOICES bounds the fleet");
    assert_eq!(r.dropped, 2);
    assert_eq!(ambient_voice_list(&mut app).len(), 32);
}

#[test]
fn teardown_sweeps_ambient_voices_with_the_session() {
    let dir = ambient_dir();
    let mut app = ambient_app(dir.path());
    ambient_car(&mut app, ambient_spec("testamb"));
    app.update();
    assert_eq!(ambient_voice_list(&mut app).len(), 1);

    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

// ---------------------------------------------------------------------------
// F07-B.7: siren programs — a SIREN_FLAG car's horn-control presses toggle the
// authored *policesiren.csv program; siren_drive walks the (play time, next
// index) chain off the session tick with one loop voice per sample.
// ---------------------------------------------------------------------------

/// A cardata horn row naming `stem` with authored `flags`.
fn flagged_car_audio(stem: &str, flags: i64) -> CarAudio {
    let csv = format!(
        "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\n{stem},0.9,{flags},1,REV,0.5\nEngine wave name,a,b\nENG,0.1,0.2\n"
    );
    CarAudio::parse(csv.as_bytes()).unwrap()
}

/// The retail tree shape: two `sirens/` player samples, one opponent
/// sample that ships only the flat `aud11` copy (the scoped lookup
/// falls back to the global stem index for it), a two-sample ping-pong
/// city program and a one-sample opponent program.
fn siren_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "aud/aud22/sirens/wail_a.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud22/sirens/wail_b.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud22/horns/testhorn.22k.wav", &pcm_wav(22050, 220));
    write(d, "aud/aud11/yelp.11k.wav", &pcm_wav(11025, 110));
    write(
        d,
        "aud/cardata/player/testcitypolicesiren.csv",
        b"Explosion sample,volume\nexplosion,0.95\nSample name,\nwail_a,0.9\nplay time,next index\n0.05,1\nSample name,\nwail_b,0.8\nplay time,next index\n0.05,0\n",
    );
    write(
        d,
        "aud/cardata/opponent/policesiren.csv",
        b"Sample name,\nyelp,\nplay time,next index\n60,0\n",
    );
    tmp
}

/// A siren program written over the fixture cardata path. `steps` is
/// `(sample name, [(play time, next index)])` in authored order.
fn write_siren_table(dir: &Path, path: &str, samples: &[(&str, &[(f32, i64)])]) {
    let mut csv = String::new();
    for (name, steps) in samples {
        csv.push_str(&format!("Sample name,\n{name},0.9\n"));
        for (t, next) in *steps {
            csv.push_str(&format!("play time,next index\n{t},{next}\n"));
        }
    }
    write(dir, path, csv.as_bytes());
}

/// A lone `Siren` state as a future AI/system activation would stamp
/// it — `Siren::activate` is the single entry point.
fn active_siren(spec: &SirenSpec, seed: u64) -> Siren {
    let mut rng = NavRng::new(seed);
    Siren::activate(spec, &mut rng, 0).unwrap()
}

fn opponent_siren_spec() -> SirenSpec {
    SirenSpec {
        explosion: None,
        samples: vec![SirenSampleSpec {
            name: "yelp".into(),
            volume: 1.0,
            steps: vec![SirenStep {
                play_time: 60.0,
                next_index: 0,
                line: 1,
            }],
        }],
    }
}

/// The audio slice a siren-flagged player sees: session `Playing`, the
/// fixture VFS/bank, the loaded `testcity` + opponent programs and the
/// chained press→toggle→drive systems `main` schedules. Ticks are
/// stepped by hand (`advance_session_tick` counts fixed steps while
/// `Playing`) so the program clock is exact in the test.
fn siren_app(dir: &Path, flags: i64) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);
    let programs = SirenAudio::load(&vfs, 1, Some("testcity"));

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_message::<HornRequest>()
        .add_systems(
            Update,
            (audio::horn_voices, audio::siren_toggle, audio::siren_drive).chain(),
        );
    if let Some(p) = programs {
        app.insert_resource(p);
    }
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleAudio {
            spec: flagged_car_audio("testhorn", flags),
        },
        SessionEntity(generation),
        Transform::default(),
    ));
    app.finish();
    app.cleanup();
    app
}

/// Step the session clock `n` fixed ticks (n/120 s of program time).
fn tick(app: &mut App, n: u64) {
    for _ in 0..n {
        app.world_mut()
            .run_system_once(advance_session_tick)
            .unwrap();
    }
}

#[test]
fn siren_tables_load_per_city_and_role() {
    let dir = siren_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    // The city stem keys the player file; the opponent file is shared.
    let sa = SirenAudio::load(&vfs, 1, Some("testcity")).unwrap();
    let player = sa.player.as_ref().unwrap();
    assert_eq!(player.samples.len(), 2);
    assert_eq!(player.samples[0].name, "wail_a");
    assert_eq!(player.explosion.as_ref().unwrap().name, "explosion");
    let opponent = sa.opponent.as_ref().unwrap();
    assert_eq!(opponent.samples.len(), 1);
    assert_eq!(opponent.samples[0].name, "yelp");
    // A city shipping no table leaves the player side empty — never
    // another city's program — while the shared side still loads.
    let sa = SirenAudio::load(&vfs, 1, Some("nocity")).unwrap();
    assert!(sa.player.is_none());
    assert!(sa.opponent.is_some());
    // And neither side resolving yields no resource at all.
    let empty = tempfile::tempdir().unwrap();
    let mut vfs2 = Vfs::new();
    vfs2.mount_dir(empty.path(), 0).unwrap();
    assert!(SirenAudio::load(&vfs2, 1, Some("testcity")).is_none());
}

#[test]
fn a_flagged_press_toggles_the_authored_program() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    let car = player(&mut app);

    app.world_mut().write_message(HornRequest);
    app.update();
    // The press counted as a horn-control press but spawned no horn
    // voice — the flag routes it to the siren toggle.
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.horns, r.sirens, r.siren_live), (1, 1, 1));
    assert_eq!(r.failed, 0);
    assert!(app.world().get::<Siren>(car).is_some());
    // One loop voice on the authored first sample, the player's own
    // non-spatial anchor (DSN-37) at the authored volume.
    let world = app.world_mut();
    let (kind, parent, settings, session_entity) = world
        .query::<(&AudioVoice, &ChildOf, &PlaybackSettings, &SessionEntity)>()
        .iter(world)
        .next()
        .map(|(v, p, s, e)| (v.kind, p.parent(), (s.mode, s.spatial, s.volume), e.0))
        .unwrap();
    assert_eq!(kind, VoiceKind::Siren);
    assert_eq!(parent, car);
    assert!(matches!(settings.0, PlaybackMode::Loop));
    assert!(!settings.1);
    assert_eq!(settings.2, Volume::Linear(0.9));
    assert_eq!(
        session_entity,
        app.world().resource::<Session>().generation()
    );

    // Second press: the toggle releases — component and voice die.
    app.world_mut().write_message(HornRequest);
    app.update();
    assert!(app.world().get::<Siren>(car).is_none());
    assert_eq!(voices(&mut app), 0);
    assert_eq!(app.world().resource::<AudioReport>().siren_live, 0);

    // Third press starts a fresh activation.
    app.world_mut().write_message(HornRequest);
    app.update();
    assert!(app.world().get::<Siren>(car).is_some());
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.horns, r.sirens, r.siren_live), (3, 2, 1));
}

#[test]
fn an_unflagged_car_keeps_the_ordinary_horn() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 0);
    app.world_mut().write_message(HornRequest);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.horns, r.sirens, r.siren_live), (1, 0, 0));
    assert_eq!(voices(&mut app), 1);
    let world = app.world_mut();
    let kind = world
        .query::<&AudioVoice>()
        .iter(world)
        .next()
        .unwrap()
        .kind;
    assert_eq!(kind, VoiceKind::Horn);
    let car = player(&mut app);
    assert!(app.world().get::<Siren>(car).is_none());
}

#[test]
fn the_drive_walks_the_authored_chain_on_the_session_tick() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    let car = player(&mut app);
    app.world_mut().write_message(HornRequest);
    app.update();

    let siren = app.world().get::<Siren>(car).unwrap();
    assert_eq!(siren.play.sample, 0);
    let first_voice = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<AudioVoice>>()
            .iter(world)
            .next()
            .unwrap()
    };

    // Six ticks = 0.05 s — the authored step time; seven leaves the
    // machine landed on sample 1.
    tick(&mut app, 7);
    app.update();
    let siren = app.world().get::<Siren>(car).unwrap();
    assert_eq!(
        siren.play.sample, 1,
        "the authored next index moved the program"
    );
    let second_voice = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<AudioVoice>>()
            .iter(world)
            .next()
            .unwrap()
    };
    assert_ne!(
        first_voice, second_voice,
        "a switch respawns the loop voice"
    );
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.sirens, r.siren_live, r.voices), (2, 1, 2));

    // The ping-pong returns to sample 0 on the next expiry.
    tick(&mut app, 7);
    app.update();
    assert_eq!(app.world().get::<Siren>(car).unwrap().play.sample, 0);
    assert_eq!(app.world().resource::<AudioReport>().sirens, 3);
}

#[test]
fn the_program_holds_while_the_session_tick_is_frozen() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    let car = player(&mut app);
    app.world_mut().write_message(HornRequest);
    app.update();
    tick(&mut app, 3);
    app.update();
    let before = app.world().get::<Siren>(car).unwrap().play.remaining;

    // No ticks → no program time: a paused session's frozen clock
    // holds the step exactly where it was.
    app.update();
    app.update();
    let after = app.world().get::<Siren>(car).unwrap().play.remaining;
    assert_eq!(before, after);
    assert_eq!(app.world().get::<Siren>(car).unwrap().play.sample, 0);
}

#[test]
fn a_missing_siren_wave_warns_once_per_activation() {
    let dir = siren_dir();
    // Neither program sample ships a wave.
    write_siren_table(
        dir.path(),
        "aud/cardata/player/testcitypolicesiren.csv",
        &[("ghost_a", &[(0.02, 1)]), ("ghost_b", &[(0.02, 0)])],
    );
    let mut app = siren_app(dir.path(), 4);
    app.world_mut().write_message(HornRequest);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.sirens, r.failed, r.siren_live), (0, 1, 1));
    assert_eq!(voices(&mut app), 0);

    // The chain keeps walking — the second stem fails too — but a
    // revisit of the first does not re-warn.
    tick(&mut app, 4);
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().failed, 2);
    tick(&mut app, 4);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 2, "the failed stem is not re-counted on revisit");
    assert_eq!(r.siren_live, 1, "the program still runs, silently");
}

#[test]
fn a_sentinel_siren_sample_is_authored_silence() {
    let dir = siren_dir();
    write_siren_table(
        dir.path(),
        "aud/cardata/player/testcitypolicesiren.csv",
        &[("NOSOUND", &[(0.02, 1)]), ("wail_a", &[(0.02, 0)])],
    );
    let mut app = siren_app(dir.path(), 4);
    app.world_mut().write_message(HornRequest);
    app.update();
    // The sentinel sample voices nothing and is not a failure.
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.sirens, r.failed, r.siren_live), (0, 0, 1));
    assert_eq!(voices(&mut app), 0);
    // The authored chain still advances into the voiced sample.
    tick(&mut app, 4);
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().sirens, 1);
    assert_eq!(voices(&mut app), 1);
}

#[test]
fn a_flagged_press_without_a_program_reports_failed() {
    let dir = siren_dir();
    std::fs::remove_file(
        dir.path()
            .join("aud/cardata/player/testcitypolicesiren.csv"),
    )
    .unwrap();
    let mut app = siren_app(dir.path(), 4);
    // The shared opponent table still resolved, so the resource
    // exists — the *player* side is the absent one.
    assert!(app.world().resource::<SirenAudio>().player.is_none());
    app.world_mut().write_message(HornRequest);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.horns, r.sirens, r.failed), (1, 0, 1));
    let car = player(&mut app);
    assert!(app.world().get::<Siren>(car).is_none());
    assert_eq!(voices(&mut app), 0);
}

#[test]
fn a_malformed_siren_table_is_no_program_not_a_substitute() {
    let dir = siren_dir();
    // A car-audio table under the siren path parses as the wrong kind —
    // it must not substitute the opponent program or another file.
    write(
        dir.path(),
        "aud/cardata/player/testcitypolicesiren.csv",
        b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nH,0.9,4,1,C,0.5\nEngine wave name,a,b\nE,0.1,0.2\n",
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let sa = SirenAudio::load(&vfs, 1, Some("testcity")).unwrap();
    assert!(sa.player.is_none());
    assert!(sa.opponent.is_some(), "the good side still loads");

    let mut app = siren_app(dir.path(), 4);
    app.world_mut().write_message(HornRequest);
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().failed, 1);
}

#[test]
fn an_opponent_siren_is_a_spatial_child_voice() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    let spec = opponent_siren_spec();
    let generation = app.world().resource::<Session>().generation();
    let opp = app
        .world_mut()
        .spawn((
            VehicleAudio {
                spec: flagged_car_audio("testhorn", 4),
            },
            active_siren(&spec, 1),
            SessionEntity(generation),
            Transform::default(),
        ))
        .id();
    app.update();

    let world = app.world_mut();
    let (_, parent, settings) = world
        .query::<(&AudioVoice, &ChildOf, &PlaybackSettings)>()
        .iter(world)
        .find(|(v, ..)| v.kind == VoiceKind::Siren)
        .map(|(v, p, s)| (v.kind, p.parent(), (s.mode, s.spatial)))
        .unwrap();
    assert_eq!(parent, opp);
    assert!(matches!(settings.0, PlaybackMode::Loop));
    assert!(settings.1, "a non-local siren is a world emitter");
    // The opponent program's `yelp` ships only the flat aud11 copy —
    // the scoped lookup fell back to the global stem index.
    let world = app.world_mut();
    let handle = world
        .query::<(&AudioVoice, &AudioPlayer<PcmAudio>)>()
        .iter(world)
        .find(|(v, ..)| v.kind == VoiceKind::Siren)
        .map(|(_, p)| p.0.clone())
        .unwrap();
    let waves = app.world().resource::<Assets<PcmAudio>>();
    assert_eq!(waves.get(&handle).unwrap().sample_rate.get(), 11025);
}

#[test]
fn the_siren_bound_drops_past_max_sirens() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    let spec = opponent_siren_spec();
    let generation = app.world().resource::<Session>().generation();
    for i in 0..8u64 {
        app.world_mut().spawn((
            VehicleAudio {
                spec: flagged_car_audio("testhorn", 4),
            },
            active_siren(&spec, i + 1),
            SessionEntity(generation),
            Transform::default(),
        ));
    }
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().siren_live, 8);

    // The ninth activation — the player's own press — is refused.
    app.world_mut().write_message(HornRequest);
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.dropped, 1);
    let car = player(&mut app);
    assert!(app.world().get::<Siren>(car).is_none());
    assert_eq!(app.world().resource::<AudioReport>().siren_live, 8);
}

#[test]
fn teardown_sweeps_siren_voices_with_the_session() {
    let dir = siren_dir();
    let mut app = siren_app(dir.path(), 4);
    app.world_mut().write_message(HornRequest);
    app.update();
    assert_eq!(voices(&mut app), 1);

    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

// ---------------------------------------------------------------------------
// F18-B.3: precipitation ambience — the authored `<name>exterior`/
// `<name>interior` beds crossfaded on the shelter probe plus the seeded
// `thunder` clap schedule, driven through `weather_voices` on a synthetic
// install. The `load_session_world` binding leg lives in `tests/precip.rs`
// alongside the particle rig's.
// ---------------------------------------------------------------------------

/// The retail rain tree: the two beds and the clap under the bank's
/// preferred aud22 tier. `stems` chooses which are present so the
/// failure leg can withhold one.
fn weather_dir(stems: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for stem in stems {
        write(
            tmp.path(),
            &format!("aud/aud22/{stem}.22k.wav"),
            &pcm_wav(22050, 220),
        );
    }
    tmp
}

const RAIN_STEMS: &[&str] = &["rainexterior", "raininterior", "thunder"];

/// The weather-audio slice of the production app: a `Playing` session,
/// the fixture WaveBank, the `WeatherAudio` resource
/// `load_session_world` inserts for a precipitating selector, one
/// active world camera (the listener's anchor — the same pick the
/// production systems make) and real physics for the shelter probe.
/// Fixed 1/60 s updates make the crossfade and the clap schedule
/// deterministic.
fn weather_app(dir: &Path, weather: u8, seed: u64) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(TransformPlugin)
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(
            Update,
            (
                audio::weather_voices,
                audio::count_sinks,
                audio::sync_audio_pause,
            ),
        );
    if let Some(ambience) = WeatherAudio::bind(Weather::new(weather).unwrap(), seed) {
        app.insert_resource(ambience);
    }
    app.world_mut()
        .spawn((Camera3d::default(), Transform::from_xyz(0.0, 1.0, 0.0)));
    app.finish();
    app.cleanup();
    app
}

/// `(role, mix)` of every live bed voice.
fn bed_mixes(app: &mut App) -> Vec<(WeatherRole, f32)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<&WeatherVoice>()
        .iter(app.world())
        .map(|b| (b.role, b.mix.volume))
        .collect();
    v.sort_by_key(|(role, _)| *role as u8);
    v
}

/// A non-precipitating selector binds no ambience at all — the
/// resource `load_session_world` inserts is `None`, so a dry session
/// carries no bed state, no schedule and no `aud=` field activity.
#[test]
fn dry_weather_binds_no_ambience() {
    for selector in [0u8, 1, 2] {
        assert!(
            WeatherAudio::bind(Weather::new(selector).unwrap(), 7).is_none(),
            "selector {selector} bound a bed"
        );
    }
}

/// A rainy session resolves both authored bed stems into looping
/// session-stamped voices — the exterior bed at its authored-constant
/// level while the listener sits in the open — and `thunder` resolves
/// without a clap inside its minimum delay.
#[test]
fn a_rainy_session_binds_the_authored_rain_beds() {
    let dir = weather_dir(RAIN_STEMS);
    let mut app = weather_app(dir.path(), 3, 7);
    app.update();
    app.update();

    let generation = app.world().resource::<Session>().generation();
    let world = app.world_mut();
    let mut voices = world.query::<(
        &AudioVoice,
        &WeatherVoice,
        &PlaybackSettings,
        &SessionEntity,
    )>();
    let all: Vec<_> = voices.iter(world).collect();
    assert_eq!(all.len(), 2, "one bed per resolved stem");
    for (voice, _, settings, stamp) in &all {
        assert_eq!(voice.kind, VoiceKind::Weather);
        assert!(matches!(settings.mode, PlaybackMode::Loop));
        assert_eq!(stamp.0, generation);
    }
    let report = app.world().resource::<AudioReport>();
    assert_eq!(report.weather, 2);
    assert_eq!(report.failed, 0);

    // Exposed listener: the mix settles at the exterior level.
    for _ in 0..30 {
        app.update();
    }
    let mixes = bed_mixes(&mut app);
    assert_eq!(mixes.len(), 2);
    assert!(
        (mixes[0].1 - 0.85).abs() < 1e-3,
        "exposed exterior bed at its level: {:?}",
        mixes
    );
    assert_eq!(mixes[1].1, 0.0, "interior bed silent in the open");
    assert!(!app.world().resource::<AudioReport>().interior);
}

/// The shelter probe: a roof over the active camera swings the
/// crossfade to the interior bed; removing it eases back. The
/// interior level is deliberately below the exterior's — the authored
/// constants the exe stores beside the stems (DSN-61).
#[test]
fn the_shelter_probe_crossfades_to_the_interior_bed() {
    let dir = weather_dir(RAIN_STEMS);
    let mut app = weather_app(dir.path(), 3, 7);
    for _ in 0..10 {
        app.update();
    }

    // A roof 8 m over the camera — inside the 64 m probe reach.
    app.world_mut().spawn((
        Collider::cuboid(60.0, 0.5, 60.0),
        Transform::from_xyz(0.0, 9.0, 0.0),
    ));
    for _ in 0..60 {
        app.update();
    }
    let mixes = bed_mixes(&mut app);
    assert!(
        (mixes[1].1 - 0.65).abs() < 1e-3,
        "sheltered interior bed at its level: {mixes:?}"
    );
    assert!(mixes[0].1 < 1e-3, "exterior bed faded out: {mixes:?}");
    assert!(app.world().resource::<AudioReport>().interior);

    // Lifting the roof eases the mix home — no latch, no snap.
    let mut roofs = app.world_mut().query_filtered::<Entity, With<Collider>>();
    let roof = roofs.single(app.world()).unwrap();
    app.world_mut().entity_mut(roof).despawn();
    for _ in 0..60 {
        app.update();
    }
    let mixes = bed_mixes(&mut app);
    assert!(
        (mixes[0].1 - 0.85).abs() < 1e-3,
        "exterior bed restored: {mixes:?}"
    );
    assert!(!app.world().resource::<AudioReport>().interior);
}

/// A stem the install does not ship counts one `failed` and never
/// retries — the resolved half still plays, and nothing is
/// substituted (F18-AC06).
#[test]
fn a_missing_bed_stem_counts_failed_once() {
    // No `raininterior` — and no `thunder` either, so both misses
    // count exactly once across the run.
    let dir = weather_dir(&["rainexterior"]);
    let mut app = weather_app(dir.path(), 3, 7);
    for _ in 0..30 {
        app.update();
    }
    let report = app.world().resource::<AudioReport>();
    assert_eq!(report.weather, 1, "the resolved bed still spawned");
    assert_eq!(report.failed, 2, "interior + thunder each counted once");
    // Under a roof the one-sided bed simply goes quiet — no
    // substitute voice.
    app.world_mut().spawn((
        Collider::cuboid(60.0, 0.5, 60.0),
        Transform::from_xyz(0.0, 9.0, 0.0),
    ));
    for _ in 0..60 {
        app.update();
    }
    let mixes = bed_mixes(&mut app);
    assert_eq!(mixes.len(), 1);
    assert!(mixes[0].1 < 1e-3, "exterior faded with no interior bed");
    assert_eq!(app.world().resource::<AudioReport>().failed, 2);
}

/// The clap schedule is seeded: the first `thunder` voice lands inside
/// the authored-constant delay window, and a second app on the same
/// seed fires on the identical update — the F18 req-5 deterministic
/// leg for the audio half.
#[test]
fn thunder_fires_on_the_seeded_schedule() {
    let dir = weather_dir(RAIN_STEMS);
    // `app.update()` is 1/60 s: the 13–15 s window lands the first
    // clap on update [780, 900).
    let first_clap = |app: &mut App| -> usize {
        for i in 1..=960 {
            app.update();
            if app.world().resource::<AudioReport>().thunder > 0 {
                return i;
            }
        }
        0
    };
    let mut a = weather_app(dir.path(), 3, 42);
    let mut b = weather_app(dir.path(), 3, 42);
    let (fa, fb) = (first_clap(&mut a), first_clap(&mut b));
    assert!(
        (780..=900).contains(&fa),
        "first clap inside the window: {fa}"
    );
    assert_eq!(fa, fb, "same seed, same clap schedule");

    // The voice is a bounded despawn one-shot stamped to the session.
    let world = a.world_mut();
    let clap = world
        .query::<(&AudioVoice, &PlaybackSettings, &SessionEntity)>()
        .iter(world)
        .find(|(v, ..)| v.kind == VoiceKind::Thunder)
        .map(|(_, s, e)| (s.mode, e.0))
        .expect("a thunder voice");
    assert!(matches!(clap.0, PlaybackMode::Despawn));
    assert_eq!(clap.1, a.world().resource::<Session>().generation());
}

/// The beds and any in-flight clap are `SessionEntity`-stamped — the
/// production teardown sweep takes them with the session.
#[test]
fn teardown_sweeps_the_weather_voices() {
    let dir = weather_dir(RAIN_STEMS);
    let mut app = weather_app(dir.path(), 3, 7);
    for _ in 0..10 {
        app.update();
    }
    assert!(voices(&mut app) >= 2);

    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

// ---------------------------------------------------------------------------
// F18-B.5: environmental pre-race commentary — the `spchdata`
// registry → speaker → cue table → wave chain, sequenced one-shots
// inside the pre-race window.
// ---------------------------------------------------------------------------

/// Noon + rainy — the effective-conditions pick the fixture's tables
/// serve (`timenoon_prerace` + `wearain_prerace`).
fn commentary_conditions() -> SessionConditions {
    SessionConditions {
        time_of_day: TimeOfDay::new(1).unwrap(),
        weather: Weather::new(3).unwrap(),
    }
}

/// A retail-shaped `spchdata` tree: the sf registry authoring two
/// announcers (SF's real five includes the table-less `as3` gap —
/// two exercises the same miss leg for less fixture), both speaker
/// dirs shipping the wearain/timenoon tables and every wave the
/// drawn suffixes can name. One-second clips so the sequencing gap
/// is measurable at 1/60 s updates.
fn commentary_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_commentary_tree(tmp.path());
    tmp
}

/// The fixture tree [`commentary_dir`] describes, written under `d`.
fn write_commentary_tree(d: &Path) {
    write(
        d,
        "aud/spchdata/sf.csv",
        b"\nNum announcers\n2\nprefix\nAS\n",
    );
    for speaker in ["as1", "as2"] {
        write(
            d,
            &format!("aud/spchdata/{speaker}/wearain_prerace.csv"),
            b"Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nWEARAIN,3,0\n",
        );
        write(
            d,
            &format!("aud/spchdata/{speaker}/timenoon_prerace.csv"),
            b"Name prefix/type header,end sufix value,sufix add value\nTIMEOFDAY header,,\nTIMENOON,2,0\n",
        );
        for stem in [
            format!("{speaker}wearain01"),
            format!("{speaker}wearain02"),
            format!("{speaker}wearain03"),
            format!("{speaker}timenoon01"),
            format!("{speaker}timenoon02"),
        ] {
            write(
                d,
                &format!("aud/aud22/{speaker}/{stem}.22k.wav"),
                &pcm_wav(22050, 22050),
            );
        }
    }
}

/// The commentary slice of the production app: a session parked at
/// `phase`, the fixture WaveBank, the `CommentaryAudio` resource
/// `load_session_world` inserts for a city session (`sf`'s authored
/// `aud/spchdata/sf.csv` registry), fixed 1/60 s updates and the one
/// system the Update schedules run.
fn commentary_app(dir: &Path, seed: u64, phase: SessionPhase) -> App {
    commentary_app_for(dir, seed, phase, None)
}

/// [`commentary_app`] for a Crash Course lesson when `lesson_table`
/// names its cue table: the binding `load_session_world` makes, plus
/// the verdict system the app schedules before the queue drains.
fn commentary_app_for(
    dir: &Path,
    seed: u64,
    phase: SessionPhase,
    lesson_table: Option<&str>,
) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    match phase {
        SessionPhase::Ready => {}
        SessionPhase::Countdown => {
            session.transition(SessionPhase::Countdown).unwrap();
        }
        SessionPhase::Playing => {
            session.transition(SessionPhase::Countdown).unwrap();
            session.transition(SessionPhase::Playing).unwrap();
        }
        _ => unreachable!("test phases stop at Playing"),
    }

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(
            Update,
            (
                mm2_app::lesson::lesson_verdict_cue.before(audio::commentary_voices),
                audio::commentary_voices,
            ),
        );
    if let Some(commentary) = CommentaryAudio::bind(Some("sf"), commentary_conditions(), seed) {
        app.insert_resource(commentary.with_lesson_table(lesson_table.map(str::to_owned)));
    }
    app.finish();
    app.cleanup();
    app
}

/// Sorted `(stamp, stem, kind, mode)` of every spawned commentary voice.
fn commentary_voices(app: &mut App) -> Vec<(u64, String, VoiceKind, PlaybackMode)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<(
            &SessionEntity,
            &CommentaryVoice,
            &AudioVoice,
            &PlaybackSettings,
        )>()
        .iter(app.world())
        .map(|(e, c, v, s)| (e.0, c.stem.clone(), v.kind, s.mode))
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// A Playing city session resolves the registry, draws one speaker
/// and plays both authored cues — weather before time-of-day — as
/// session-stamped despawn one-shots.
#[test]
fn a_city_session_plays_the_authored_weather_and_time_cues() {
    let dir = commentary_dir();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    // 1 s clips + a 0.25 s gap at 1/60 s updates — ~80 updates play
    // the whole queue.
    for _ in 0..90 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.commentary, 2, "weather + time-of-day cues");
    assert_eq!(r.failed, 0);
    let generation = app.world().resource::<Session>().generation();
    let played = commentary_voices(&mut app);
    assert_eq!(played.len(), 2);
    for (stamp, stem, kind, mode) in &played {
        assert_eq!(*stamp, generation);
        assert_eq!(*kind, VoiceKind::Commentary);
        assert!(matches!(mode, PlaybackMode::Despawn));
        assert!(stem.starts_with("as"), "a drawn speaker dir: {stem}");
    }
    // One cue family each — the drawn suffix stays inside the
    // authored ranges (wearain 1..=3, timenoon 1..=2).
    let mut kinds: Vec<&str> = played
        .iter()
        .map(|(_, s, ..)| {
            if s.contains("wearain") {
                "wearain"
            } else if s.contains("timenoon") {
                "timenoon"
            } else {
                s.as_str()
            }
        })
        .collect();
    kinds.sort();
    assert_eq!(kinds, ["timenoon", "wearain"]);
}

/// Commentary is a pre-race binding: parked at `Ready` nothing
/// resolves; `Countdown` opens the window and the queue plays.
#[test]
fn commentary_waits_for_the_pre_race_window() {
    let dir = commentary_dir();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Ready);
    for _ in 0..10 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed, r.voices), (0, 0, 0));

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Countdown)
        .unwrap();
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(
        r.commentary, 1,
        "the first cue fires on the window's first frame"
    );
    assert_eq!(r.failed, 0);
}

/// The queue sequences, not stacks: cue two waits out the first
/// clip's decoded duration plus the gap rather than speaking over it.
#[test]
fn the_second_cue_waits_for_the_first_clip() {
    let dir = commentary_dir();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    app.update();
    assert_eq!(app.world().resource::<AudioReport>().commentary, 1);
    // ~1 s clip + 0.25 s gap at 1/60 s: still one voice at 70 updates.
    for _ in 0..70 {
        app.update();
    }
    assert_eq!(app.world().resource::<AudioReport>().commentary, 1);
    for _ in 0..60 {
        app.update();
    }
    assert_eq!(app.world().resource::<AudioReport>().commentary, 2);
}

/// A city with no `spchdata` registry counts one failure — once,
/// never per frame — and plays nothing.
#[test]
fn a_missing_registry_counts_once_and_never_retries() {
    let dir = commentary_dir();
    std::fs::remove_file(dir.path().join("aud/spchdata/sf.csv")).unwrap();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    for _ in 0..30 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 1);
    assert_eq!((r.commentary, r.voices), (0, 0));
}

/// A drawn speaker whose dir ships no tables (SF's authored `as3`
/// gap) counts each bound cue's miss and substitutes nothing.
#[test]
fn an_authored_speaker_gap_counts_each_cue() {
    let dir = commentary_dir();
    // Find the seed that lands the speaker draw on `as2`, then strip
    // as2's tables — the authored-gap shape.
    let mut gap_seed = None;
    for seed in 0..40 {
        let mut probe = commentary_app(dir.path(), seed, SessionPhase::Playing);
        probe.update();
        let played = commentary_voices(&mut probe);
        if played.iter().any(|(_, s, ..)| s.starts_with("as2")) {
            gap_seed = Some(seed);
            break;
        }
    }
    let seed = gap_seed.expect("some seed draws as2");
    std::fs::remove_dir_all(dir.path().join("aud/spchdata/as2")).unwrap();
    let mut app = commentary_app(dir.path(), seed, SessionPhase::Playing);
    for _ in 0..90 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 2, "both bound cue tables are absent");
    assert_eq!((r.commentary, r.voices), (0, 0));
}

/// A cue whose table resolves but whose wave does not counts one
/// failure and stays silent — the other family still plays; nothing
/// is substituted.
#[test]
fn a_missing_wave_counts_and_is_never_substituted() {
    let dir = commentary_dir();
    for speaker in ["as1", "as2"] {
        for n in 1..=3 {
            std::fs::remove_file(
                dir.path()
                    .join(format!("aud/aud22/{speaker}/{speaker}wearain0{n}.22k.wav")),
            )
            .unwrap();
        }
    }
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    for _ in 0..90 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 1, "the weather cue's wave resolve failed");
    assert_eq!(r.commentary, 1, "the time cue still plays");
    let played = commentary_voices(&mut app);
    assert!(played.iter().all(|(_, s, ..)| s.contains("timenoon")));
}

/// A modded table whose `add` sits past `end` (here `i64::MAX`)
/// resolves to an undrawable range — one counted failure through the production
/// resolve path, never an overflow panic — and the other cue family
/// still plays.
#[test]
fn an_overflowing_sufix_range_counts_failed_not_panics() {
    let dir = commentary_dir();
    for speaker in ["as1", "as2"] {
        write(
            dir.path(),
            &format!("aud/spchdata/{speaker}/wearain_prerace.csv"),
            format!(
                "Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nWEARAIN,3,{}\n",
                i64::MAX
            )
            .as_bytes(),
        );
    }
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    for _ in 0..90 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 1, "the overflowing weather cue is undrawable");
    assert_eq!(r.commentary, 1, "the time cue still plays");
    assert_eq!(commentary_voices(&mut app).len(), 1);
}

/// The speaker and suffix draws ride the seeded stream — two sessions
/// on one seed play identical stems, in identical order.
#[test]
fn the_cue_draw_replays_identically_for_the_same_seed() {
    let dir = commentary_dir();
    let played = |seed: u64| {
        let mut app = commentary_app(dir.path(), seed, SessionPhase::Playing);
        for _ in 0..90 {
            app.update();
        }
        commentary_voices(&mut app)
            .into_iter()
            .map(|(_, stem, ..)| stem)
            .collect::<Vec<_>>()
    };
    assert_eq!(played(42), played(42));
    // And the pick really is a draw: across seeds both authored
    // speakers get heard (the registry authors two).
    let mut speakers: std::collections::BTreeSet<String> = Default::default();
    for seed in 0..20 {
        for stem in played(seed) {
            speakers.insert(stem[..3].to_string());
        }
    }
    assert!(speakers.contains("as1") && speakers.contains("as2"));
}

/// The queue's voices are `SessionEntity`-stamped — the production
/// teardown sweep takes any still-playing cue with the session.
#[test]
fn teardown_sweeps_commentary_voices() {
    let dir = commentary_dir();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    for _ in 0..90 {
        app.update();
    }
    assert!(commentary_voices(&mut app).len() == 2);

    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert_eq!(voices(&mut app), 0);
}

/// A dev world binds no commentary — `load_session_world` passes no
/// city for a non-city mode and the resource is simply absent, so
/// the session carries no speech state at all.
#[test]
fn a_dev_world_binds_no_commentary() {
    assert!(CommentaryAudio::bind(None, commentary_conditions(), 7).is_none());
}

/// The race-effect fixture plus the announcer tree and each speaker's
/// `checkpoint.csv`/`circuit.csv` closing-gate line (`RACECHECK`, suffix `1..=2`)
/// and `RESULTS*` tiers (`blitz.csv` has no middle tier).
fn final_checkpoint_dir() -> tempfile::TempDir {
    let tmp = race_effect_fixture();
    write_commentary_tree(tmp.path());
    for speaker in ["as1", "as2"] {
        for table in ["checkpoint", "circuit"] {
            write(
                tmp.path(),
                &format!("aud/spchdata/{speaker}/{table}.csv"),
                b"Name prefix/type header,end sufix value,sufix add value\nFINALCHECKPOINT header,,\nRACECHECK,2,0\nRESULTSPOOR header,,\nRESULTPOOR,1,0\nRESULTSMID header,,\nRESULTMID,1,0\nRESULTSWIN header,,\nRESULTWIN,1,0\n",
            );
        }
        // Blitz authors two tiers only, like the retail table.
        write(
            tmp.path(),
            &format!("aud/spchdata/{speaker}/blitz.csv"),
            b"Name prefix/type header,end sufix value,sufix add value\nRESULTSPOOR header,,\nRESULTPOOR,1,0\nRESULTSWIN header,,\nRESULTWIN,1,0\n",
        );
        for n in 1..=2 {
            write(
                tmp.path(),
                &format!("aud/aud22/{speaker}/{speaker}racecheck0{n}.22k.wav"),
                &pcm_wav(22050, 22050),
            );
        }
        for tier in ["poor", "mid", "win"] {
            write(
                tmp.path(),
                &format!("aud/aud22/{speaker}/{speaker}result{tier}01.22k.wav"),
                &pcm_wav(22050, 22050),
            );
        }
    }
    tmp
}

/// The race-effect app with the session's commentary bound the way
/// `load_session_world` binds it for a race event of `table`.
fn final_checkpoint_app(dir: &Path, table: Option<&'static str>) -> (App, Entity) {
    let (mut app, player) = race_effect_app(dir);
    // Speech is sequenced on the clock, so step it like the other
    // commentary fixtures do.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    app.add_systems(Update, audio::commentary_voices);
    let bound = CommentaryAudio::bind(Some("sf"), commentary_conditions(), 7)
        .unwrap()
        .with_event_table(table);
    app.insert_resource(bound);
    (app, player)
}

/// Run the race and clear one of its two gates, so one checkpoint is
/// left to cross.
fn clear_all_but_one_checkpoint(app: &mut App, player: Entity) {
    use mm2_game::{RacePhase, RaceProgress, RaceState};
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Running;
    app.update();
    assert_eq!(
        app.world().resource::<AudioReport>().commentary,
        1,
        "the first pre-race cue only: nothing is announced before the gate"
    );
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 0, 0, 1, 0);
}

/// F08-A: the announcer's `FINALCHECKPOINT` line is spoken once one
/// checkpoint is left — in the speaker the pre-race cues drew, queued
/// behind the pre-race speech instead of over it, while the session is
/// still Playing.
#[test]
fn the_last_checkpoint_to_cross_is_announced_once() {
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("checkpoint"));
    clear_all_but_one_checkpoint(&mut app, player);
    for _ in 0..400 {
        app.update();
    }
    assert!(race_effect_stems(&mut app).contains(&"waypoint"));
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.failed, 0);
    assert_eq!(r.commentary, 3, "weather, time of day, closing gate");
    let spoken: Vec<String> = commentary_voices(&mut app)
        .into_iter()
        .map(|(_, stem, ..)| stem)
        .collect();
    let gate: Vec<&String> = spoken.iter().filter(|s| s.contains("racecheck")).collect();
    assert_eq!(gate.len(), 1, "{spoken:?}");
    let speaker = &gate[0][..3];
    assert!(
        spoken.iter().all(|s| s.starts_with(speaker)),
        "one announcer per session: {spoken:?}"
    );
    assert!(["01", "02"].iter().any(|n| gate[0].ends_with(n)));
    // A held edge never repeats the line.
    for _ in 0..200 {
        app.update();
    }
    assert_eq!(app.world().resource::<AudioReport>().commentary, 3);
}

/// Under `Ordered` only the final lap's closing gate is the final
/// checkpoint — the same gate on an earlier lap is not announced.
#[test]
fn only_the_final_laps_closing_gate_is_announced() {
    use mm2_game::{CheckpointRule, RacePhase, RaceProgress, RaceState};
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("circuit"));
    {
        let mut race = app.world_mut().resource_mut::<RaceState>();
        race.phase = RacePhase::Running;
        race.definition.rule = CheckpointRule::Ordered;
        race.definition.laps = 2;
    }
    app.update();
    // Lap 1 of 2, heading for the last gate of the lap: not final.
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 1, 0, 1, 0);
    for _ in 0..200 {
        app.update();
    }
    assert_eq!(app.world().resource::<AudioReport>().commentary, 2);
    // Lap 2 of 2, same gate: final.
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 1, 1, 3, 0);
    for _ in 0..200 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (3, 0));
    assert!(
        commentary_voices(&mut app)
            .iter()
            .any(|(_, s, ..)| s.contains("racecheck"))
    );
}

/// A session that is not a race event binds no table, so the same edge
/// asks for nothing and counts nothing.
#[test]
fn an_unbound_session_announces_no_final_checkpoint() {
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), None);
    clear_all_but_one_checkpoint(&mut app, player);
    for _ in 0..400 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (2, 0), "only the pre-race cues");
    assert!(
        !commentary_voices(&mut app)
            .iter()
            .any(|(_, s, ..)| s.contains("racecheck"))
    );
}

/// A table without the section (or a speaker without the table) is one
/// counted miss, never retried per frame and never replaced by another
/// line.
#[test]
fn a_missing_closing_gate_line_counts_once() {
    let dir = final_checkpoint_dir();
    for speaker in ["as1", "as2"] {
        write(
            dir.path(),
            &format!("aud/spchdata/{speaker}/checkpoint.csv"),
            b"Name prefix/type header,end sufix value,sufix add value\nPRERACE header,,\nPRE,19,0\n",
        );
    }
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("checkpoint"));
    clear_all_but_one_checkpoint(&mut app, player);
    for _ in 0..400 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (2, 1));
}

/// Finish the local participant of a `field`-car race with `ahead` rivals
/// in front (`outcome` is `finished`, `timed-out`, or anything else for
/// a finish the ledger never ranked), recording the ledger standings the production finish writes,
/// and move the session to Results the way the finish does.
fn end_race(app: &mut App, player: Entity, field: usize, outcome: &str, ahead: u32) {
    use mm2_game::{ParticipantState, RaceProgress, ResultLedger, SessionOutcome, SessionResult};
    app.init_resource::<ResultLedger>();
    let local = app.world().get::<Player>(player).unwrap().id;
    let progress = app.world().get::<RaceProgress>(player).unwrap().clone();
    for n in 1..field {
        let id = app.world_mut().resource_mut::<Session>().mint_player_id();
        app.world_mut().spawn((
            Player {
                id,
                control: PlayerControl::Remote,
            },
            progress.clone(),
        ));
        if (n as u32) <= ahead {
            let result = app.world_mut().resource_mut::<Session>().mint_result_id(id);
            let generation = app.world().resource::<Session>().generation();
            assert_eq!(result.generation, generation);
            app.world_mut()
                .resource_mut::<ResultLedger>()
                .record(SessionResult {
                    id: result,
                    tick: 1,
                    outcome: SessionOutcome::Finished { race_ticks: 500 },
                })
                .unwrap();
        }
    }
    let result = app
        .world_mut()
        .resource_mut::<Session>()
        .mint_result_id(local);
    let (state, recorded) = match outcome {
        "finished" => (
            ParticipantState::Finished {
                race_ticks: 900,
                result: result.clone(),
            },
            Some(SessionOutcome::Finished { race_ticks: 900 }),
        ),
        "timed-out" => (
            ParticipantState::TimedOut {
                race_ticks: 900,
                result: result.clone(),
            },
            Some(SessionOutcome::TimedOut { race_ticks: 900 }),
        ),
        // A finish the ledger never ranked.
        _ => (
            ParticipantState::Finished {
                race_ticks: 900,
                result: result.clone(),
            },
            None,
        ),
    };
    if let Some(outcome) = recorded {
        app.world_mut()
            .resource_mut::<ResultLedger>()
            .record(SessionResult {
                id: result,
                tick: 2,
                outcome,
            })
            .unwrap();
    }
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .state = state;
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Results)
        .unwrap();
}

/// The race-end line read back: the stems the announcer spoke that are
/// results lines.
fn spoken_results(app: &mut App) -> Vec<String> {
    commentary_voices(app)
        .into_iter()
        .map(|(_, stem, ..)| stem)
        .filter(|s| s.contains("result"))
        .collect()
}

/// F08-A: the announcer reads a tier of the table's `RESULTS*` sections
/// when the race ends, from the speaker the pre-race cues drew, once —
/// first place wins, last of a field is poor, between is middling, and
/// a time-out is poor.
#[test]
fn the_announcer_reads_the_results_tier_the_standing_earns() {
    for (field, outcome, ahead, tier) in [
        (3, "finished", 0, "win"),
        (3, "finished", 1, "mid"),
        (3, "finished", 2, "poor"),
        (3, "timed-out", 0, "poor"),
        // A lone racer who finishes won.
        (1, "finished", 0, "win"),
    ] {
        let dir = final_checkpoint_dir();
        let (mut app, player) = final_checkpoint_app(dir.path(), Some("checkpoint"));
        app.update();
        end_race(&mut app, player, field, outcome, ahead);
        for _ in 0..400 {
            app.update();
        }
        let spoken = spoken_results(&mut app);
        assert_eq!(spoken.len(), 1, "{field} {outcome} {ahead}: {spoken:?}");
        assert!(
            spoken[0].ends_with(&format!("result{tier}01")),
            "{spoken:?}"
        );
        let r = app.world().resource::<AudioReport>();
        assert_eq!(r.failed, 0, "{field} {outcome} {ahead}");
        let all: Vec<String> = commentary_voices(&mut app)
            .into_iter()
            .map(|(_, stem, ..)| stem)
            .collect();
        assert!(
            all.iter().all(|s| s.starts_with(&spoken[0][..3])),
            "one announcer per session: {all:?}"
        );
    }
}

/// A table with no `RESULTSMID` (blitz) has two tiers: a middling
/// finish reads the poor one, a win still reads the win line.
#[test]
fn a_table_without_a_middle_tier_reads_poor_for_a_middling_finish() {
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("blitz"));
    app.update();
    end_race(&mut app, player, 3, "finished", 1);
    for _ in 0..400 {
        app.update();
    }
    let spoken = spoken_results(&mut app);
    assert_eq!(spoken.len(), 1, "{spoken:?}");
    assert!(spoken[0].ends_with("resultpoor01"), "{spoken:?}");
    assert_eq!(app.world().resource::<AudioReport>().failed, 0);
}

/// A finish the ledger never ranked has no standing to announce, a
/// Crash Course/cruise session (no table) speaks nothing at all, and a
/// held Results phase never repeats the line.
#[test]
fn an_unranked_or_unbound_finish_announces_no_results() {
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("checkpoint"));
    app.update();
    end_race(&mut app, player, 2, "unranked", 0);
    for _ in 0..200 {
        app.update();
    }
    assert!(spoken_results(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().failed, 0);

    let (mut app, player) = final_checkpoint_app(dir.path(), None);
    app.update();
    end_race(&mut app, player, 2, "finished", 0);
    for _ in 0..200 {
        app.update();
    }
    assert!(spoken_results(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().failed, 0);
}

/// A session that reaches Results without ever running the pre-race
/// window drew no announcer: the verdict is one counted miss, and the
/// pre-race resolve is not run late.
#[test]
fn a_session_with_no_announcer_counts_one_results_miss() {
    let dir = final_checkpoint_dir();
    let (mut app, player) = final_checkpoint_app(dir.path(), Some("checkpoint"));
    end_race(&mut app, player, 2, "finished", 0);
    for _ in 0..200 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (0, 1));
}

/// The tier a standing earns, edges included.
#[test]
fn a_standing_maps_to_its_results_tier() {
    use mm2_app::audio::ResultsTier;
    use mm2_app::audio::ResultsTier::{Mid, Poor, Win};
    let tier = ResultsTier::for_standing;
    assert_eq!(tier(None, 4), None);
    assert_eq!(tier(Some(1), 4), Some(Win));
    assert_eq!(tier(Some(2), 4), Some(Mid));
    assert_eq!(tier(Some(3), 4), Some(Mid));
    assert_eq!(tier(Some(4), 4), Some(Poor));
    assert_eq!(tier(Some(1), 1), Some(Win));
    assert_eq!(tier(Some(2), 2), Some(Poor));
}

// ---------------------------------------------------------------------------
// F21-B.16: a Crash Course lesson's instructor — the school's own cue
// table (`aud/spchdata/ccl/ccl3.csv`) speaks the intro ahead of the
// city's environmental lines, then the verdict.
// ---------------------------------------------------------------------------

const LESSON_TABLE: &str = "aud/spchdata/ccl/ccl3.csv";

/// [`commentary_dir`] plus a lesson table authoring `CCL03INTRO` (3),
/// `CCL03FAIL` (4) and `CCL03SUCC` (2) and the waves they name, under
/// the flat `aud11/ccl` dir the retail install ships them in.
fn lesson_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_commentary_tree(d);
    write(
        d,
        LESSON_TABLE,
        b"Name prefix/type header,end sufix value,sufix add value\n\
          PRERACE header,,\nCCL03INTRO,3,0\n\
          RESULTSPOOR header,,\nCCL03FAIL,4,0\n\
          RESULTSWIN header,,\nCCL03SUCC,2,0\n",
    );
    for (name, count) in [("intro", 3), ("fail", 4), ("succ", 2)] {
        for n in 1..=count {
            write(
                d,
                &format!("aud/aud11/ccl/ccl03{name}{n:02}.11k.wav"),
                &pcm_wav(11025, 11025),
            );
        }
    }
    tmp
}

fn lesson_driver() -> mm2_app::lesson::LessonDriver {
    use mm2_content::{LessonLeg, LessonObjective};
    use mm2_game::{
        Checkpoint, CheckpointRule, EventKey, EventTableKind, LessonRun, RaceDefinition,
    };
    let leg = LessonLeg {
        filename: "slalom".into(),
        objective: LessonObjective::Maneuver,
        source: "crash3:slalom".into(),
        definition: RaceDefinition {
            checkpoints: vec![Checkpoint {
                center: Vec3::ZERO,
                radius: 10.0,
                height: 5.0,
                heading_deg: 0.0,
                require_direction: false,
            }],
            finish: None,
            rule: CheckpointRule::Ordered,
            laps: 1,
            time_limit_ticks: None,
            params: mm2_game::EventParams::default(),
            countdown_ticks: 3,
            start_slots: Vec::new(),
        },
    };
    mm2_app::lesson::LessonDriver::new(mm2_app::race::LessonSetup {
        key: EventKey {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            stem: "lesson1".into(),
        },
        run: LessonRun::new(1).unwrap(),
        legs: vec![leg],
        rewards: Default::default(),
        availability: Default::default(),
        aimap: None,
    })
}

fn leg_outcome(finished: bool) -> mm2_game::ParticipantState {
    let result = mm2_game::ResultId {
        generation: 1,
        participant: mm2_game::PlayerId(0),
        event: None,
        sequence: 0,
    };
    if finished {
        mm2_game::ParticipantState::Finished {
            race_ticks: 100,
            result,
        }
    } else {
        mm2_game::ParticipantState::TimedOut {
            race_ticks: 100,
            result,
        }
    }
}

/// The instructor's intro is the first line a lesson speaks — ahead of
/// the weather and time-of-day lines the city announcer adds — and
/// nothing is missed.
#[test]
fn a_lesson_opens_with_the_instructors_intro() {
    let dir = lesson_dir();
    let mut app = commentary_app_for(dir.path(), 7, SessionPhase::Playing, Some(LESSON_TABLE));
    for _ in 0..3 {
        app.update();
    }
    let first = commentary_voices(&mut app);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].1.starts_with("ccl03intro"), "{first:?}");
    for _ in 0..400 {
        app.update();
    }
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (3, 0), "intro + weather + time");
}

/// A session that is not a lesson speaks no instructor line even when
/// the table ships, and a lesson verdict is refused without a table.
#[test]
fn a_non_lesson_session_has_no_instructor() {
    let dir = lesson_dir();
    let mut app = commentary_app(dir.path(), 7, SessionPhase::Playing);
    for _ in 0..400 {
        app.update();
    }
    let said = commentary_voices(&mut app);
    assert!(
        said.iter().all(|(_, s, ..)| !s.starts_with("ccl")),
        "{said:?}"
    );
    assert_eq!(app.world().resource::<AudioReport>().commentary, 2);

    use mm2_app::audio::EventCue;
    let mut bound = CommentaryAudio::bind(Some("sf"), commentary_conditions(), 7).unwrap();
    assert!(!bound.request(EventCue::LessonPass));
    let mut bound = bound.with_lesson_table(Some(LESSON_TABLE.into()));
    assert!(bound.request(EventCue::LessonFail));
    assert!(
        !bound.request(EventCue::LessonPass),
        "one verdict per attempt"
    );
    // A lesson does not open the event-kind announcer.
    assert!(!bound.request(EventCue::FinalCheckpoint));
}

/// The lesson's pass reads the table's `RESULTSWIN` line, its failure
/// the `RESULTSPOOR` one, each once however many frames the verdict
/// stays standing.
#[test]
fn a_lesson_verdict_speaks_the_matching_line_once() {
    for (pass, prefix, waves) in [(true, "ccl03succ", 2), (false, "ccl03fail", 4)] {
        let dir = lesson_dir();
        let mut app = commentary_app_for(dir.path(), 7, SessionPhase::Playing, Some(LESSON_TABLE));
        let mut driver = lesson_driver();
        // Undecided: the lesson says nothing yet.
        app.insert_resource(lesson_driver());
        for _ in 0..400 {
            app.update();
        }
        assert_eq!(app.world().resource::<AudioReport>().commentary, 3);

        driver.observe(&leg_outcome(pass));
        app.insert_resource(driver);
        for _ in 0..400 {
            app.update();
        }
        let r = app.world().resource::<AudioReport>();
        assert_eq!((r.commentary, r.failed), (4, 0), "pass={pass}");
        let verdict: Vec<_> = commentary_voices(&mut app)
            .into_iter()
            .filter(|(_, s, ..)| s.starts_with("ccl03") && !s.contains("intro"))
            .collect();
        assert_eq!(verdict.len(), 1, "{verdict:?}");
        let n: usize = verdict[0].1[prefix.len()..].parse().unwrap();
        assert!(verdict[0].1.starts_with(prefix) && (1..=waves).contains(&n));
    }
}

/// Original-content validation (opt-in: `MM2_RETAIL` names an install;
/// reports "not run" otherwise). Every Crash Course row of both cities
/// must resolve to a readable lesson table authoring a `PRERACE`, a
/// `RESULTSPOOR` and a `RESULTSWIN` line, and every wave those lines
/// can draw is looked up in the real wave bank. Structural gaps fail
/// the test; draws that name a wave the install does not ship are
/// authored quirks, listed (never filtered out of the denominator).
#[test]
fn every_retail_lesson_resolves_its_instructor_lines() {
    use mm2_assets::{InstallMount, mount_install};
    use mm2_formats::spchdata::CueTable;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("MM2_RETAIL unset: retail lesson speech sweep NOT run");
        return;
    };
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
    let mut bank = WaveBank::index(&vfs);
    let mut waves = Assets::<PcmAudio>::default();
    let (mut rows, mut lines, mut draws) = (0, 0, 0);
    let mut structural = Vec::new();
    let mut missing = Vec::new();
    for city in ["london", "sf"] {
        let catalog = mm2_content::EventCatalog::scan(&vfs, city);
        let crash: Vec<usize> = catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == mm2_game::EventTableKind::CrashCourse)
            .map(|e| e.event_ref.index)
            .collect();
        assert!(!crash.is_empty(), "{city}: no Crash Course rows");
        for row in crash {
            rows += 1;
            let label = format!("{city} row {row}");
            let Some(path) = mm2_game::lesson_speech_table(city, row) else {
                structural.push(format!("{label}: no lesson table binding"));
                continue;
            };
            let table = vfs
                .read_logical(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    CueTable::parse(&String::from_utf8_lossy(&b)).map_err(|e| e.to_string())
                });
            let table = match table {
                Ok(t) => t,
                Err(e) => {
                    structural.push(format!("{label}: {path}: {e}"));
                    continue;
                }
            };
            for section in ["PRERACE", "RESULTSPOOR", "RESULTSWIN"] {
                lines += 1;
                let Some(cue) = table.section(section).and_then(|s| s.rows.first()) else {
                    structural.push(format!("{label}: {path} authors no {section}"));
                    continue;
                };
                for n in (cue.add + 1)..=cue.end {
                    draws += 1;
                    let stem = mm2_game::cue_wave_stem("", &cue.prefix, n);
                    if let Err(e) = bank.load(&vfs, &mut waves, &stem) {
                        missing.push(format!("{label} {section}: {stem}: {e}"));
                    }
                }
            }
        }
    }
    eprintln!(
        "retail lesson speech: {rows} rows, {lines} lines, {draws} drawable waves, {} missing:\n{}",
        missing.len(),
        missing.join("\n")
    );
    assert_eq!(rows, 26, "13 crash rows per school");
    assert!(structural.is_empty(), "{structural:#?}");
    // Measured 2026-10-08: three failure-line draws name a wave the
    // install does not ship (`end` counts the family's files there,
    // not its top suffix). A change in this count is new evidence.
    assert_eq!(missing.len(), 3, "{missing:#?}");
}

/// Each race-end cue is asked for once: a second tier is refused.
#[test]
fn a_results_request_is_accepted_once_whatever_the_tier() {
    use mm2_app::audio::{EventCue, ResultsTier};
    let mut bound = CommentaryAudio::bind(Some("sf"), commentary_conditions(), 7)
        .unwrap()
        .with_event_table(Some("checkpoint"));
    assert!(bound.request(EventCue::Results(ResultsTier::Mid)));
    assert!(!bound.request(EventCue::Results(ResultsTier::Win)));
    assert!(bound.request(EventCue::FinalCheckpoint));
}

/// `request` accepts each cue once and only with a bound table.
#[test]
fn a_final_checkpoint_request_is_accepted_once_and_only_when_bound() {
    use mm2_app::audio::EventCue;
    let mut bound = CommentaryAudio::bind(Some("sf"), commentary_conditions(), 7)
        .unwrap()
        .with_event_table(Some("blitz"));
    assert!(bound.request(EventCue::FinalCheckpoint));
    assert!(!bound.request(EventCue::FinalCheckpoint));
    let mut unbound = CommentaryAudio::bind(Some("sf"), commentary_conditions(), 7).unwrap();
    assert!(!unbound.request(EventCue::FinalCheckpoint));
}

fn race_effect_app(dir: &Path) -> (App, Entity) {
    use mm2_game::{Checkpoint, CheckpointRule, RaceDefinition, RaceProgress, RaceState};
    let mut app = horn_app(dir, false);
    let def = RaceDefinition {
        checkpoints: [0.0, 50.0]
            .map(|x| Checkpoint {
                center: Vec3::new(x, 0.0, 0.0),
                radius: 5.0,
                height: 8.0,
                heading_deg: 0.0,
                require_direction: false,
            })
            .to_vec(),
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: Some(20 * mm2_game::RACE_TICK_HZ),
        params: default(),
        countdown_ticks: 360,
        start_slots: vec![],
    };
    let generation = app.world().resource::<Session>().generation();
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let player = app
        .world_mut()
        .spawn((
            Player {
                id,
                control: PlayerControl::Local,
            },
            RaceProgress::new(&def),
        ))
        .id();
    app.insert_resource(RaceState::new(def, generation))
        .add_systems(Update, mm2_app::race_audio::race_cue_voices);
    (app, player)
}

fn race_effect_fixture() -> tempfile::TempDir {
    let tmp = fixture_dir();
    for stem in [
        "startracelow",
        "startracehigh",
        "waypoint",
        "lastwaypoint",
        "endofracetag",
        "timerwarning",
        "youlose",
    ] {
        write(
            tmp.path(),
            &format!("aud/aud22/{stem}.22k.wav"),
            &pcm_wav(22050, 220),
        );
    }
    tmp
}

fn race_effect_stems(app: &mut App) -> Vec<&'static str> {
    app.world_mut()
        .query::<&mm2_app::race_audio::RaceCueVoice>()
        .iter(app.world())
        .map(|voice| voice.stem)
        .collect()
}

#[test]
fn race_effects_countdown_checkpoints_warning_and_finish_once() {
    use mm2_game::{ParticipantState, RacePhase, RaceProgress, RaceState};
    let tmp = race_effect_fixture();
    let (mut app, player) = race_effect_app(tmp.path());
    app.update();
    app.update();
    assert_eq!(race_effect_stems(&mut app), ["startracelow"]);
    for remaining in [240, 120] {
        app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Countdown { remaining };
        app.update();
    }
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Running;
    app.update();
    assert_eq!(
        race_effect_stems(&mut app)
            .iter()
            .filter(|s| **s == "startracelow")
            .count(),
        3
    );
    assert!(race_effect_stems(&mut app).contains(&"startracehigh"));
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 0, 0, 1, 0);
    app.update();
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(3, 0, 0, 2, 0);
    app.update();
    assert!(race_effect_stems(&mut app).contains(&"waypoint"));
    assert!(race_effect_stems(&mut app).contains(&"lastwaypoint"));
    app.world_mut().resource_mut::<RaceState>().clock = 10 * u64::from(mm2_game::RACE_TICK_HZ);
    app.update();
    app.update();
    assert_eq!(
        race_effect_stems(&mut app)
            .iter()
            .filter(|s| **s == "timerwarning")
            .count(),
        1
    );
    let id = app.world().get::<Player>(player).unwrap().id;
    let result = app.world_mut().resource_mut::<Session>().mint_result_id(id);
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .state = ParticipantState::Finished {
        race_ticks: 1200,
        result,
    };
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Results)
        .unwrap();
    app.update();
    app.update();
    assert_eq!(
        race_effect_stems(&mut app)
            .iter()
            .filter(|s| **s == "endofracetag")
            .count(),
        1
    );
    for (owner, settings) in app.world_mut().query_filtered::<(&SessionEntity, &PlaybackSettings), With<mm2_app::race_audio::RaceCueVoice>>().iter(app.world()) {
        assert_eq!(owner.0, app.world().resource::<Session>().generation());
        assert!(matches!(settings.mode, PlaybackMode::Despawn));
        assert!(!settings.spatial);
    }
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Unloading)
        .unwrap();
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    assert!(race_effect_stems(&mut app).is_empty());
}

#[test]
fn race_effects_hold_during_pause_ignore_remote_progress_and_scope_to_restart() {
    use mm2_game::{RacePhase, RaceProgress, RaceState};
    let tmp = race_effect_fixture();
    let (mut app, player) = race_effect_app(tmp.path());
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Running;
    app.update();
    let remote_id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let mut remote_progress = app.world().get::<RaceProgress>(player).unwrap().clone();
    remote_progress.apply_replicated(3, 0, 0, 2, 0);
    app.world_mut().spawn((
        Player {
            id: remote_id,
            control: PlayerControl::Remote,
        },
        remote_progress,
    ));
    app.update();
    assert!(race_effect_stems(&mut app).is_empty());
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 0, 0, 1, 0);
    app.update();
    assert!(race_effect_stems(&mut app).is_empty());
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    app.update();
    app.update();
    assert_eq!(race_effect_stems(&mut app), ["waypoint"]);
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Unloading)
        .unwrap();
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    app.update();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Menu)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .begin(SessionConfig::default())
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Ready)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    let generation = app.world().resource::<Session>().generation();
    app.world_mut().resource_mut::<RaceState>().generation = generation;
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(0, 0, 0, 0, 0);
    app.update();
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 0, 0, 1, 0);
    app.update();
    assert_eq!(race_effect_stems(&mut app), ["waypoint"]);
}

#[test]
fn race_effects_missing_clip_is_counted_once_without_retries() {
    use mm2_game::{RacePhase, RaceProgress, RaceState};
    let tmp = fixture_dir();
    let (mut app, player) = race_effect_app(tmp.path());
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Running;
    app.update();
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 0, 0, 1, 0);
    app.update();
    app.update();
    assert!(race_effect_stems(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().failed, 1);
}

#[test]
fn race_effects_circuit_route_progress_and_timeout_use_distinct_cues() {
    use mm2_game::{CheckpointRule, ParticipantState, RacePhase, RaceProgress, RaceState};
    let tmp = race_effect_fixture();
    let (mut app, player) = race_effect_app(tmp.path());
    {
        let mut race = app.world_mut().resource_mut::<RaceState>();
        race.phase = RacePhase::Running;
        race.definition.rule = CheckpointRule::Ordered;
        race.definition.laps = 2;
    }
    app.update();
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 1, 0, 1, 0);
    app.update();
    assert_eq!(race_effect_stems(&mut app), ["waypoint"]);
    // Route-based clears are credited through the same authoritative progress.
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .apply_replicated(1, 1, 1, 1, 2);
    app.update();
    assert_eq!(race_effect_stems(&mut app), ["waypoint", "lastwaypoint"]);
    let id = app.world().get::<Player>(player).unwrap().id;
    let result = app.world_mut().resource_mut::<Session>().mint_result_id(id);
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .state = ParticipantState::TimedOut {
        race_ticks: 2400,
        result,
    };
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Complete;
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Results)
        .unwrap();
    app.update();
    app.update();
    assert_eq!(
        race_effect_stems(&mut app),
        ["waypoint", "lastwaypoint", "youlose"]
    );
}

// ---------------------------------------------------------------------------
// F20-B.1 — a chasing cop's siren follows its emergency lights
// ---------------------------------------------------------------------------

/// `siren_app` plus the system that turns a cop's `EmergencyLights`
/// into its siren, ordered ahead of the drive as `main` chains it.
fn cop_siren_app(dir: &Path) -> App {
    let mut app = siren_app(dir, 4);
    app.add_systems(
        Update,
        audio::siren_follow_lights
            .before(audio::siren_toggle)
            .before(audio::siren_drive),
    );
    app
}

fn spawn_cop(app: &mut App, flags: i64) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .spawn((
            VehicleAudio {
                spec: flagged_car_audio("testhorn", flags),
            },
            SessionEntity(generation),
            Transform::default(),
        ))
        .id()
}

fn siren_voice_parents(app: &mut App) -> Vec<Entity> {
    let world = app.world_mut();
    world
        .query::<(&AudioVoice, &ChildOf)>()
        .iter(world)
        .filter(|(v, _)| v.kind == VoiceKind::Siren)
        .map(|(_, p)| p.parent())
        .collect()
}

#[test]
fn a_cops_emergency_lights_start_and_stop_the_opponent_siren() {
    let dir = siren_dir();
    let mut app = cop_siren_app(dir.path());
    let cop = spawn_cop(&mut app, 4);
    app.update();
    assert!(
        app.world().get::<Siren>(cop).is_none(),
        "quiet until called"
    );
    assert_eq!(app.world().resource::<AudioReport>().sirens, 0);

    app.world_mut()
        .entity_mut(cop)
        .insert(mm2_game::EmergencyLights::default());
    app.update();
    assert!(app.world().get::<Siren>(cop).is_some());
    assert_eq!(siren_voice_parents(&mut app), vec![cop]);
    // The shared opponent program (`yelp`, the flat aud11 copy), a
    // world emitter — not the player's city program.
    let world = app.world_mut();
    let (handle, spatial) = world
        .query::<(&AudioVoice, &AudioPlayer<PcmAudio>, &PlaybackSettings)>()
        .iter(world)
        .find(|(v, ..)| v.kind == VoiceKind::Siren)
        .map(|(_, p, s)| (p.0.clone(), s.spatial))
        .unwrap();
    assert!(spatial);
    let waves = app.world().resource::<Assets<PcmAudio>>();
    assert_eq!(waves.get(&handle).unwrap().sample_rate.get(), 11025);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.sirens, r.siren_live, r.failed), (1, 1, 0));

    // Steady lights do not respawn the loop voice.
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(app.world().resource::<AudioReport>().sirens, 1);

    // Lights out: the component and its loop voice go together.
    app.world_mut()
        .entity_mut(cop)
        .remove::<mm2_game::EmergencyLights>();
    app.update();
    assert!(app.world().get::<Siren>(cop).is_none());
    assert!(siren_voice_parents(&mut app).is_empty());
    assert_eq!(app.world().resource::<AudioReport>().siren_live, 0);

    // A second chase starts a fresh activation.
    app.world_mut()
        .entity_mut(cop)
        .insert(mm2_game::EmergencyLights::default());
    app.update();
    assert_eq!(siren_voice_parents(&mut app), vec![cop]);
    assert_eq!(app.world().resource::<AudioReport>().sirens, 2);
}

#[test]
fn a_car_without_the_siren_flag_stays_silent_under_lights() {
    let dir = siren_dir();
    let mut app = cop_siren_app(dir.path());
    let car = spawn_cop(&mut app, 0);
    app.world_mut()
        .entity_mut(car)
        .insert(mm2_game::EmergencyLights::default());
    app.update();
    assert!(app.world().get::<Siren>(car).is_none());
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.sirens, r.failed, r.dropped), (0, 0, 0));
}

#[test]
fn the_player_car_is_never_started_by_emergency_lights() {
    // The horn toggle owns the player's siren; lights on a player car
    // (nothing writes them there) do not start the program.
    let dir = siren_dir();
    let mut app = cop_siren_app(dir.path());
    let car = player(&mut app);
    app.world_mut()
        .entity_mut(car)
        .insert(mm2_game::EmergencyLights::default());
    app.update();
    assert!(app.world().get::<Siren>(car).is_none());
}

#[test]
fn chasing_cops_past_the_siren_bound_drop_and_stay_lit_silent() {
    let dir = siren_dir();
    let mut app = cop_siren_app(dir.path());
    let cops: Vec<Entity> = (0..10).map(|_| spawn_cop(&mut app, 4)).collect();
    for cop in &cops {
        app.world_mut()
            .entity_mut(*cop)
            .insert(mm2_game::EmergencyLights::default());
    }
    app.update();
    let r = app.world().resource::<AudioReport>();
    assert_eq!(r.siren_live, 8);
    assert_eq!(r.dropped, 2, "the ninth and tenth are refused");
    let sirened = cops
        .iter()
        .filter(|c| app.world().get::<Siren>(**c).is_some())
        .count();
    assert_eq!(sirened, 8);
    // A refused cop retries next pass (and is counted again) rather
    // than being silently forgotten — drop is per attempt.
    app.world_mut()
        .entity_mut(cops[0])
        .remove::<mm2_game::EmergencyLights>();
    app.update();
    assert_eq!(
        app.world().resource::<AudioReport>().siren_live,
        8,
        "a freed slot is taken by a waiting cop"
    );
}

#[test]
fn a_session_with_no_opponent_program_counts_the_failure_not_a_substitute() {
    let dir = siren_dir();
    std::fs::remove_file(dir.path().join("aud/cardata/opponent/policesiren.csv")).unwrap();
    let mut app = cop_siren_app(dir.path());
    let cop = spawn_cop(&mut app, 4);
    app.world_mut()
        .entity_mut(cop)
        .insert(mm2_game::EmergencyLights::default());
    app.update();
    assert!(app.world().get::<Siren>(cop).is_none());
    assert!(siren_voice_parents(&mut app).is_empty());
    assert!(app.world().resource::<AudioReport>().failed >= 1);
}

/// F23 volume levels: the horn app with the production `level_new_voices`
/// pass and the user's settings resource.
fn leveled_horn_app(dir: &Path, audio: AudioLevels) -> App {
    let mut app = horn_app(dir, false);
    app.insert_resource(GraphicsSettings { audio, ..default() })
        .add_systems(
            PostUpdate,
            audio::level_new_voices.before(bevy::transform::TransformSystems::Propagate),
        );
    app
}

/// Fire the horn once and read the one-shot's initial playback volume.
fn horn_volume(app: &mut App) -> f32 {
    press_enter(app);
    app.update();
    app.update();
    release_enter(app);
    let world = app.world_mut();
    let volumes: Vec<Volume> = world
        .query_filtered::<&PlaybackSettings, With<AudioVoice>>()
        .iter(world)
        .map(|s| s.volume)
        .collect();
    assert_eq!(volumes.len(), 1, "exactly one horn voice spawned");
    volumes[0].to_linear()
}

#[test]
fn default_levels_play_a_voice_at_its_authored_volume() {
    let dir = fixture_dir();
    let mut app = leveled_horn_app(dir.path(), AudioLevels::default());
    // `car_audio` authors the horn at 0.9.
    assert!((horn_volume(&mut app) - 0.9).abs() < 1e-6);
}

#[test]
fn master_and_bus_levels_scale_a_new_voice_once() {
    let dir = fixture_dir();
    let levels = AudioLevels {
        master: 50,
        effects: 40,
        ..AudioLevels::default()
    };
    let mut app = leveled_horn_app(dir.path(), levels);
    let expected = 0.9 * 0.5 * 0.4;
    assert!((horn_volume(&mut app) - expected).abs() < 1e-6);
    // Later frames must not scale the same voice again.
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let now: Vec<f32> = world
        .query_filtered::<&PlaybackSettings, With<AudioVoice>>()
        .iter(world)
        .map(|s| s.volume.to_linear())
        .collect();
    assert!(now.iter().all(|v| (v - expected).abs() < 1e-6), "{now:?}");
}

#[test]
fn a_zeroed_bus_silences_its_voices_only() {
    let dir = fixture_dir();
    let levels = AudioLevels {
        effects: 0,
        ..AudioLevels::default()
    };
    let mut app = leveled_horn_app(dir.path(), levels);
    assert_eq!(horn_volume(&mut app), 0.0, "the horn is a sound effect");
    // The commentary bus is untouched, and a non-voice entity's
    // playback is never scaled.
    let speech = app
        .world_mut()
        .spawn((
            AudioVoice {
                kind: VoiceKind::Commentary,
            },
            PlaybackSettings {
                volume: Volume::Linear(0.8),
                ..default()
            },
        ))
        .id();
    let bystander = app
        .world_mut()
        .spawn(PlaybackSettings {
            volume: Volume::Linear(0.8),
            ..default()
        })
        .id();
    app.update();
    for entity in [speech, bystander] {
        let volume = app.world().get::<PlaybackSettings>(entity).unwrap().volume;
        assert!((volume.to_linear() - 0.8).abs() < 1e-6);
    }
}

#[test]
fn an_app_without_settings_plays_the_authored_mix() {
    let dir = fixture_dir();
    let mut app = horn_app(dir.path(), false);
    app.add_systems(
        PostUpdate,
        audio::level_new_voices.before(bevy::transform::TransformSystems::Propagate),
    );
    assert!((horn_volume(&mut app) - 0.9).abs() < 1e-6);
}

#[test]
fn every_voice_kind_has_a_bus_and_the_world_sounds_share_one() {
    use mm2_app::settings::AudioBus;
    for (kind, bus) in [
        (VoiceKind::Engine, AudioBus::Effects),
        (VoiceKind::Siren, AudioBus::Effects),
        (VoiceKind::RaceCue, AudioBus::Effects),
        (VoiceKind::Commentary, AudioBus::Commentary),
        (VoiceKind::Weather, AudioBus::City),
        (VoiceKind::Thunder, AudioBus::City),
        (VoiceKind::Object, AudioBus::City),
    ] {
        assert_eq!(kind.bus(), bus, "{kind:?}");
    }
}

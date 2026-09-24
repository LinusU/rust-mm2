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

use bevy::audio::{AudioPlayer, PlaybackMode, PlaybackSettings};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use mm2_app::audio::{
    self, AudioReport, AudioVoice, EngineVoice, HornRequest, PcmAudio, VoiceKind, WaveBank,
    decode_wave,
};
use mm2_assets::Vfs;
use mm2_formats::cardata::CarAudio;
use mm2_game::{
    DevOverrides, Mm2Vfs, PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase,
    VehicleAudio, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleState};

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

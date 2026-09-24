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
    self, AudioReport, AudioVoice, HornRequest, PcmAudio, VoiceKind, WaveBank, decode_wave,
};
use mm2_assets::Vfs;
use mm2_formats::cardata::CarAudio;
use mm2_game::{
    DevOverrides, Mm2Vfs, PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase,
    VehicleAudio, despawn_session_entities,
};

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
/// session `WaveBank`, and the same Update systems `main`/`headless`
/// schedule. One `PlayerVehicle` carries `VehicleAudio` like
/// `load_session_world` stamps it.
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

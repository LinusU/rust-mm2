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

use avian3d::prelude::LinearVelocity;
use bevy::audio::{AudioPlayer, PlaybackMode, PlaybackSettings, SpatialListener, Volume};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use mm2_app::audio::{
    self, AmbientEngineVoice, AmbientRig, AudioReport, AudioVoice, EngineVoice, GearWatch,
    HornRequest, ImpactAudio, PcmAudio, Siren, SirenAudio, SurfaceAudio, SurfaceRig, SurfaceRole,
    SurfaceVoice, VoiceKind, WaveBank, decode_wave,
};
use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::cardata::{AmbientEngine, CarAudio, SirenStep};
use mm2_formats::materials::{MaterialMap, MaterialSet};
use mm2_game::{
    AmbientAudio, AmbientEngineSpec, Banger, BangerDefinition, DevOverrides, ImpactEvent, ImpactId,
    Mm2Vfs, NavRng, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerVehicle, Session,
    SessionConfig, SessionEntity, SessionPhase, SirenSampleSpec, SirenSpec, SurfaceMaterial,
    SurfaceState, VehicleAudio, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{DriveDirection, VehicleConfig, VehicleState, vehicle_bundle};

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
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
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
fn a_remote_participant_spawns_no_voice() {
    let dir = impact_dir();
    let mut app = impact_app(dir.path());
    let (remote, _) = spawn_test_car(&mut app, PlayerControl::Remote, 1300.0);
    write_impact(&mut app, remote, ObjectId::WORLD, 30.0);
    app.update();
    // The remote car's audio belongs to its own client — nothing
    // plays here, nothing counts as a failure.
    assert!(impact_voices(&mut app).is_empty());
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.impacts, r.failed), (0, 0));
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

/// The surface fixture: a player-side `default_surfacedry.csv` whose
/// row 0 is the `_default` road (NOSOUND rolling + two slippage bands)
/// and row 1 is `grass` (a rolling loop + one wide band), plus every
/// wave they name — at distinguishing sample rates so a voice's
/// resolved asset identifies its authored row.
fn surface_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for (stem, rate) in [
        ("roadskid1", 22050),
        ("roadskid2", 32000),
        ("grassskid", 11025),
        ("rollwave", 48000),
    ] {
        write(
            d,
            &format!("aud/aud22/surfaces/{stem}.22k.wav"),
            &pcm_wav(rate, 220),
        );
    }
    write(
        d,
        "aud/cardata/player/default_surfacedry.csv",
        b"Tunnel sound index\n0\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
NOSOUND,125,0,0,0,0,0.5,0.88,2\n\
skid wave,min slippage,max slippage\n\
ROADSKID1,0.5,0.75\n\
ROADSKID2,0.75,1\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
ROLLWAVE,25,0.35,0.75,0.85,1.25,0.5,0.72,1\n\
skid wave,min slippage,max slippage\n\
GRASSSKID,0.25,1\n",
    );
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
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    // Production inserts the resource only when the authored table
    // loads — an absent record leaves none and degrades to silence.
    let table = SurfaceAudio::load(&vfs);

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

#[test]
fn an_unresolvable_surface_and_an_absent_table_stay_silent() {
    // A collider whose material index the table cannot answer.
    let dir = surface_dir();
    let mut app = surface_app(dir.path());
    let (car, collider) = surface_car(&mut app, true, SurfaceMaterial::Authored(9));
    set_contact(&mut app, car, Some(collider), 8.0, 0.6 * 0.16);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(voices(&mut app), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.voices, r.failed), (0, 0), "no fabricated row");

    // No authored table at all — `load` refuses and the system idles.
    let dir = fixture_dir();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    assert!(SurfaceAudio::load(&vfs).is_none());
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

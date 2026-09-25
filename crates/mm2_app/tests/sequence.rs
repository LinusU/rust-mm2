//! F07-AC02 scripted-sequence evidence: the `--seq` driver walks a
//! staged idle → accelerate → coast → brake → reverse program through
//! the production `VehicleInput` path and banks a drivetrain +
//! engine-mix + clutch sample at every stage boundary, which the
//! headless record prints as `seq=`.
//!
//! The dev-world leg runs the real physics sim end to end; the
//! component-level legs drive `VehicleState` directly (the same style
//! the audio suite uses) so the boundary attribution is exact.

use std::path::Path;

use bevy::prelude::*;
use mm2_app::audio::{self, AudioReport, PcmAudio, WaveBank};
use mm2_app::sequence::{self, SequenceDrive};
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, Driver, SmokeStatus};
use mm2_assets::Vfs;
use mm2_formats::cardata::CarAudio;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, Mm2Vfs, PlayerVehicle, RaceDefinition, RacePhase,
    RaceStart, RaceState, Session, SessionConfig, SessionEntity, SessionPhase,
};
use mm2_vehicle::{DriveDirection, VehicleConfig, VehicleInput, VehicleState};

/// A minimal 16-bit mono PCM RIFF/WAVE at `rate` with `frames` frames.
fn pcm_wav(rate: u32, frames: usize) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
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

/// Engine voices plus the clutch stem the cardata row names.
fn audio_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for stem in ["eidle", "edrive", "emid", "ehigh", "rev"] {
        write(
            d,
            &format!("aud/aud22/engines/{stem}.22k.wav"),
            &pcm_wav(22050, 220),
        );
    }
    tmp
}

/// `vpbug`-shaped values for one canonical fade-window row.
const FADE_VALUES: &str = "0.55,0.835,1,800,2500,7000,0.85,2,1,7000";

/// A cardata table whose engine rows ride the canonical fade-window
/// header; `REV` is the authored clutch one-shot.
fn audio_car() -> CarAudio {
    let csv = format!(
        "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nTESTHORN,0.9,0,4,REV,0.5\nEngine wave name,Min Volume,Max Volume,fade in start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\nEIDLE,{FADE_VALUES}\nEDRIVE,0.55,0.9,500,2800,6500,10000,0.6,2.5,500,12000\nEMID,0.55,0.85,500,4000,7000,11000,1,2.25,500,11000\nEHIGH,0.55,0.91,3000,8000,15000,15000,0.65,2.25,3000,12000\n"
    );
    CarAudio::parse(csv.as_bytes()).unwrap()
}

/// The audio + sequence slice of the production app: a `Playing`
/// session, the fixture VFS/bank, the engine rig + clutch systems and
/// `sequence_drive` ordered after `clutch_voices` exactly as the real
/// schedules place it. The spawned player carries `VehicleAudio` like
/// `load_session_world` stamps it; no physics runs — the test writes
/// `VehicleState` the way the sim would report it.
fn seq_app(dir: &Path) -> App {
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
        .insert_resource(SequenceDrive::default())
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_systems(
            Update,
            (
                audio::engine_rigs,
                audio::engine_drive,
                audio::clutch_voices,
                sequence::sequence_drive.after(audio::clutch_voices),
            ),
        );
    app.world_mut().spawn((
        PlayerVehicle,
        mm2_game::VehicleAudio { spec: audio_car() },
        VehicleInput::default(),
        VehicleState::new(&VehicleConfig::default()),
        SessionEntity(generation),
        Transform::default(),
    ));
    app.finish();
    app.cleanup();
    app
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

fn player(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .unwrap()
}

fn set_state(app: &mut App, rpm: f32, gear: usize, direction: DriveDirection, fwd: f32) {
    let car = player(app);
    let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
    state.rpm = rpm;
    state.gear = gear;
    state.direction = direction;
    state.forward_speed = fwd;
}

fn input(app: &mut App) -> VehicleInput {
    let car = player(app);
    *app.world().get::<VehicleInput>(car).unwrap()
}

fn samples(app: &App) -> Vec<String> {
    app.world()
        .resource::<SequenceDrive>()
        .samples
        .iter()
        .map(|s| s.stage.to_string())
        .collect()
}

/// The full program drives the real sim end to end: the `seq=` record
/// must carry all five stages in order with the drivetrain evidence
/// the AC02 sequence names — idle at the authored idle band, a higher
/// RPM under load, decay on the coast, a stopped hand-off, and the
/// reverse band running backwards.
#[test]
fn the_sequence_drives_the_staged_program_on_the_dev_world() {
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        2100,
        Driver::Sequence,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "the sequence run passes the ordinary criteria: {}",
        rec.line()
    );
    let line = rec.line();
    assert!(
        line.contains("driver=sequence"),
        "record names the driver: {line}"
    );
    let seq = line
        .split_whitespace()
        .find_map(|f| f.strip_prefix("seq="))
        .expect("a Sequence run reports seq=");
    // `name:{rpm}r/{F|R}{gear}/{fwd}m/{vol}v/{pitch}p[+{n}c]` per stage.
    let parse = |entry: &str| -> (String, f32, String, f32) {
        let (name, rest) = entry.split_once(':').expect("stage:name");
        let mut it = rest.split('/');
        let rpm: f32 = it.next().unwrap().trim_end_matches('r').parse().unwrap();
        let dir_gear = it.next().unwrap().to_string();
        let fwd: f32 = it.next().unwrap().trim_end_matches('m').parse().unwrap();
        (name.to_string(), rpm, dir_gear, fwd)
    };
    let stages: Vec<_> = seq.split(',').map(parse).collect();
    let names: Vec<_> = stages.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(
        names,
        ["idle", "acc", "coast", "brake", "rev"],
        "every stage banks a sample in order: {seq}"
    );
    // The audio assertions live in the component-level tests (the dev
    // car carries no authored audio — `0.00v` is the honest report);
    // here the drivetrain evidence is what the sim actually did.
    assert!(
        stages[1].1 > stages[0].1,
        "accelerate revs past idle: {} -> {}",
        stages[0].1,
        stages[1].1
    );
    assert!(
        stages[2].1 < stages[1].1,
        "coast decays off the accel rpm: {} -> {}",
        stages[1].1,
        stages[2].1
    );
    assert!(
        stages[3].3.abs() <= 0.5,
        "the brake stage ends near-stopped: {} m/s",
        stages[3].3
    );
    assert!(
        stages[4].2.starts_with('R') && stages[4].3 < 0.0,
        "the reverse stage runs the reverse band: {}",
        seq.split(',').nth(4).unwrap()
    );
}

/// The countdown lock is the same gate every driver honors: while the
/// race holds input the program cannot advance — zero input, no stage
/// progress, no samples — and release starts the idle stage.
#[test]
fn the_countdown_lock_holds_the_program() {
    let dir = audio_dir();
    let mut app = seq_app(dir.path());
    let generation = app.world().resource::<Session>().generation();
    // A race counting down inside the Playing session — the production
    // countdown shape (Session stays Playing; RaceState::Countdown
    // holds the input lock).
    app.world_mut().insert_resource(RaceState {
        definition: RaceDefinition {
            checkpoints: vec![
                Checkpoint {
                    center: Vec3::ZERO,
                    radius: 5.0,
                    height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
                    heading_deg: 0.0,
                    require_direction: false,
                },
                Checkpoint {
                    center: Vec3::new(100.0, 0.0, 0.0),
                    radius: 5.0,
                    height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
                    heading_deg: 0.0,
                    require_direction: false,
                },
            ],
            finish: None,
            rule: CheckpointRule::AnyOrder,
            laps: 1,
            time_limit_ticks: None,
            params: EventParams::default(),
            countdown_ticks: 600,
            start_slots: vec![RaceStart {
                position: Vec3::ZERO,
                yaw_deg: None,
            }],
        },
        phase: RacePhase::Countdown { remaining: 600 },
        clock: 0,
        generation,
    });
    run(&mut app, 30);
    let vi = input(&mut app);
    assert_eq!(
        (vi.throttle, vi.brake),
        (0.0, 0.0),
        "a locked session writes zeroed input"
    );
    assert!(
        samples(&app).is_empty(),
        "the lock cannot burn the idle stage"
    );

    // Release: the countdown ends and the program starts.
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Running;
    run(&mut app, 200);
    let vi = input(&mut app);
    assert_eq!(vi.throttle, 1.0, "post-release the program accelerates");
    assert_eq!(samples(&app), ["idle"], "the idle stage banked first");
}

/// Each boundary banks the drivetrain the stage ended on plus the
/// loudest loop's computed mix and the clutch one-shots the stage
/// produced — the `seq=` evidence fields.
#[test]
fn stage_boundaries_bank_the_drivetrain_mix_and_clutch_counts() {
    let dir = audio_dir();
    let mut app = seq_app(dir.path());
    run(&mut app, 2); // rig builds; GearWatch learns (gear 0, Forward)

    // Idle (180 driving frames) → Accelerate.
    run(&mut app, 181);
    let vi = input(&mut app);
    assert_eq!(vi.throttle, 1.0, "accelerate holds full throttle");
    {
        let seq = app.world().resource::<SequenceDrive>();
        let idle = &seq.samples[0];
        assert_eq!(idle.stage, "idle");
        assert_eq!(idle.direction, DriveDirection::Forward);
        assert!(idle.mix_volume > 0.0, "the idle band is audible");
        assert_eq!(idle.clutch, 0, "first sight is not a shift");
    }

    // Accelerate (420 frames): the gearbox committing 0 -> 2 is a
    // shift — the clutch one-shot lands in this stage's delta because
    // `sequence_drive` runs after `clutch_voices`.
    set_state(&mut app, 5200.0, 2, DriveDirection::Forward, 38.0);
    run(&mut app, 421);
    let vi = input(&mut app);
    assert_eq!((vi.throttle, vi.brake), (0.0, 0.0), "coast lifts off");
    {
        let seq = app.world().resource::<SequenceDrive>();
        let acc = &seq.samples[1];
        assert_eq!(acc.stage, "acc");
        assert_eq!((acc.gear, acc.direction), (2, DriveDirection::Forward));
        assert!(
            acc.mix_speed > seq.samples[0].mix_speed,
            "the loudest loop pitches up with rpm: {} -> {}",
            seq.samples[0].mix_speed,
            acc.mix_speed
        );
        assert!(acc.clutch >= 1, "the committed upshift fired: {acc:?}");
    }

    // Coast (240): the 2 -> 1 downshift is a committed change too.
    set_state(&mut app, 1400.0, 1, DriveDirection::Forward, 9.0);
    run(&mut app, 241);
    let vi = input(&mut app);
    assert_eq!(vi.brake, 1.0, "the brake stage holds the pedal");
    {
        let seq = app.world().resource::<SequenceDrive>();
        let coast = &seq.samples[2];
        assert_eq!(coast.stage, "coast");
        assert!(coast.clutch >= 1, "the downshift fired: {coast:?}");
        assert!(
            coast.mix_speed < seq.samples[1].mix_speed,
            "the loop pitches back down off load"
        );
    }

    // Brake waits for the nearly-stopped band: still moving at the cap
    // of a short run, the stage holds.
    set_state(&mut app, 900.0, 1, DriveDirection::Forward, 0.4);
    run(&mut app, 30);
    assert!(
        samples(&app).len() == 3,
        "brake holds while the car still moves"
    );
    set_state(&mut app, 900.0, 1, DriveDirection::Forward, 0.2);
    run(&mut app, 2);
    assert_eq!(samples(&app)[3], "brake", "the stopped edge hands off");

    // Reverse (300): the held brake is now the reverse throttle, and
    // the committed Forward -> Reverse direction change fires the
    // clutch again.
    set_state(&mut app, 1800.0, 0, DriveDirection::Reverse, -3.0);
    run(&mut app, 301);
    let seq = app.world().resource::<SequenceDrive>();
    assert_eq!(seq.samples.len(), 5);
    let rev = &seq.samples[4];
    assert_eq!(rev.stage, "rev");
    assert_eq!(rev.direction, DriveDirection::Reverse);
    assert!(rev.forward_speed < 0.0);
    assert!(rev.clutch >= 1, "the direction change fired: {rev:?}");
    let vi = input(&mut app);
    assert_eq!((vi.throttle, vi.brake), (0.0, 0.0), "Done parks the input");
}

/// A mid-run session restart begins a fresh program: the fresh car has
/// not run the early stages, so the machine resets rather than
/// mislabeling the record with the old session's samples.
#[test]
fn a_restart_starts_the_program_over() {
    let dir = audio_dir();
    let mut app = seq_app(dir.path());
    run(&mut app, 181);
    assert_eq!(samples(&app), ["idle"]);

    // The session's own teardown/begin cycle bumps the generation.
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Unloading).unwrap();
        session.transition(SessionPhase::Menu).unwrap();
        session.begin(SessionConfig::default()).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update();
    {
        let seq = app.world().resource::<SequenceDrive>();
        assert!(
            seq.samples.is_empty(),
            "the new generation cleared the old program's samples"
        );
    }
    // The program restarts at idle — the next banked sample is a fresh
    // idle, not a continuation of the old program.
    run(&mut app, 181);
    assert_eq!(samples(&app), ["idle"]);
}
